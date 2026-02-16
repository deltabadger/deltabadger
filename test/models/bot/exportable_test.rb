require 'test_helper'

class Bot::ExportableTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

  setup do
    Bot::UpdateMetricsJob.stubs(:perform_later)
    @bot = create(:dca_single_asset)
  end

  # == Export ==

  test 'orders_csv returns CSV with correct headers' do
    csv = @bot.orders_csv
    parsed = CSV.parse(csv)

    assert_equal ['Timestamp', 'Order ID', 'Type', 'Side', 'Amount', 'Value', 'Price',
                  'Base Asset', 'Quote Asset', 'Status'], parsed.first
  end

  test 'orders_csv includes only submitted closed transactions' do
    create(:transaction, bot: @bot, external_id: 'closed-1', status: :submitted, external_status: :closed)
    create(:transaction, bot: @bot, external_id: 'open-1', status: :submitted, external_status: :open)
    create(:transaction, :failed, bot: @bot)

    csv = @bot.reload.orders_csv
    rows = CSV.parse(csv, headers: true)

    assert_equal 1, rows.size
    assert_equal 'closed-1', rows.first['Order ID']
  end

  test 'orders_csv formats order type and side correctly' do
    create(:transaction, bot: @bot, order_type: :market_order, side: :buy,
           external_status: :closed, status: :submitted)

    rows = CSV.parse(@bot.reload.orders_csv, headers: true)

    assert_equal 'Market', rows.first['Type']
    assert_equal 'Buy', rows.first['Side']
  end

  test 'orders_csv uses amount_exec when available' do
    create(:transaction, bot: @bot, amount: 1.0, amount_exec: 0.99,
           external_status: :closed, status: :submitted)

    rows = CSV.parse(@bot.reload.orders_csv, headers: true)

    assert_equal '0.99', rows.first['Amount']
  end

  test 'orders_csv falls back to amount when amount_exec is nil' do
    create(:transaction, bot: @bot, amount: 1.0, amount_exec: nil,
           external_status: :closed, status: :submitted)

    rows = CSV.parse(@bot.reload.orders_csv, headers: true)

    assert_equal '1.0', rows.first['Amount']
  end

  test 'orders_csv uses quote_amount_exec for value' do
    create(:transaction, bot: @bot, quote_amount: 50.0, quote_amount_exec: 49.5,
           amount: 1.0, price: 50_000,
           external_status: :closed, status: :submitted)

    rows = CSV.parse(@bot.reload.orders_csv, headers: true)

    assert_equal '49.5', rows.first['Value']
  end

  test 'orders_csv calculates value from amount * price when quote amounts nil' do
    create(:transaction, bot: @bot, quote_amount: nil, quote_amount_exec: nil,
           amount: 0.001, price: 50_000,
           external_status: :closed, status: :submitted)

    rows = CSV.parse(@bot.reload.orders_csv, headers: true)

    assert_equal 50.0, rows.first['Value'].to_f
  end

  test 'orders_csv converts timestamp to user time zone' do
    @bot.user.update!(time_zone: 'Tokyo')
    freeze_time do
      create(:transaction, bot: @bot, external_status: :closed, status: :submitted,
             created_at: Time.utc(2025, 1, 15, 12, 0, 0))

      rows = CSV.parse(@bot.reload.orders_csv, headers: true)
      timestamp = Time.zone.parse(rows.first['Timestamp'])

      assert_equal 'Tokyo', @bot.user.time_zone
      assert_equal Time.find_zone('Tokyo').parse('2025-01-15 21:00:00'), timestamp
    end
  end

  # == Import ==

  test 'import_orders_csv imports valid CSV' do
    csv = generate_csv([
      ['2025-01-15 12:00:00', 'order-1', 'Market', 'Buy', '0.001', '50', '50000',
       @bot.base_asset.symbol, @bot.quote_asset.symbol, 'closed']
    ])

    result = @bot.import_orders_csv(csv)

    assert result[:success]
    assert_equal 1, result[:imported_count]
    assert_equal 0, result[:skipped_existing]
    assert_equal 1, @bot.transactions.count
  end

  test 'import_orders_csv creates transactions with correct attributes' do
    csv = generate_csv([
      ['2025-01-15 12:00:00', 'order-1', 'Market', 'Buy', '0.001', '50', '50000',
       'BTC', 'USD', 'closed']
    ])

    @bot.import_orders_csv(csv)
    txn = @bot.transactions.last

    assert_equal "imported_#{@bot.id}_order-1", txn.external_id
    assert_equal 'market_order', txn.order_type
    assert_equal 'buy', txn.side
    assert_in_delta 0.001, txn.amount.to_f
    assert_in_delta 0.001, txn.amount_exec.to_f
    assert_in_delta 50, txn.quote_amount.to_f
    assert_in_delta 50, txn.quote_amount_exec.to_f
    assert_in_delta 50_000, txn.price.to_f
    assert_equal 'BTC', txn.base
    assert_equal 'USD', txn.quote
    assert_equal 'submitted', txn.status
    assert_equal 'closed', txn.external_status
  end

  test 'import_orders_csv handles limit orders' do
    csv = generate_csv([
      ['2025-01-15 12:00:00', 'order-1', 'Limit', 'Sell', '0.5', '25000', '50000',
       'BTC', 'USD', 'closed']
    ])

    @bot.import_orders_csv(csv)
    txn = @bot.transactions.last

    assert_equal 'limit_order', txn.order_type
    assert_equal 'sell', txn.side
  end

  test 'import_orders_csv imports multiple rows in bulk' do
    rows = 100.times.map do |i|
      ['2025-01-15 12:00:00', "order-#{i}", 'Market', 'Buy', '0.001', '50', '50000',
       'BTC', 'USD', 'closed']
    end
    csv = generate_csv(rows)

    result = @bot.import_orders_csv(csv)

    assert result[:success]
    assert_equal 100, result[:imported_count]
    assert_equal 100, @bot.transactions.count
  end

  test 'import_orders_csv skips already imported orders' do
    csv = generate_csv([
      ['2025-01-15 12:00:00', 'order-1', 'Market', 'Buy', '0.001', '50', '50000',
       'BTC', 'USD', 'closed']
    ])

    @bot.import_orders_csv(csv)
    csv.rewind
    result = @bot.import_orders_csv(csv)

    assert result[:success]
    assert_equal 0, result[:imported_count]
    assert_equal 1, result[:skipped_existing]
    assert_equal 1, @bot.transactions.count
  end

  test 'import_orders_csv skips currency mismatch rows' do
    csv = generate_csv([
      ['2025-01-15 12:00:00', 'order-1', 'Market', 'Buy', '1', '3000', '3000',
       'ETH', 'EUR', 'closed']
    ])

    result = @bot.import_orders_csv(csv)

    assert_not result[:success]
    assert_includes result[:error], 'Currency mismatch'
    assert_equal 0, @bot.transactions.count
  end

  test 'import_orders_csv skips non-closed orders' do
    csv = generate_csv([
      ['2025-01-15 12:00:00', 'order-1', 'Market', 'Buy', '0.001', '50', '50000',
       'BTC', 'USD', 'open'],
      ['2025-01-15 12:00:00', 'order-2', 'Market', 'Buy', '0.001', '50', '50000',
       'BTC', 'USD', 'cancelled'],
      ['2025-01-15 12:00:00', 'order-3', 'Market', 'Buy', '0.001', '50', '50000',
       'BTC', 'USD', 'closed']
    ])

    result = @bot.import_orders_csv(csv)

    assert result[:success]
    assert_equal 1, result[:imported_count]
  end

  test 'import_orders_csv rejects invalid CSV headers' do
    file = StringIO.new("Bad,Headers\nfoo,bar\n")

    result = @bot.import_orders_csv(file)

    assert_not result[:success]
    assert_equal I18n.t('bot.details.stats.import_invalid_format'), result[:error]
  end

  test 'import_orders_csv handles malformed CSV' do
    file = StringIO.new("not,a,proper\ncsv\"file")

    result = @bot.import_orders_csv(file)

    assert_not result[:success]
    assert_equal I18n.t('bot.details.stats.import_malformed_csv'), result[:error]
  end

  test 'import_orders_csv triggers UpdateMetricsJob once after import' do
    Bot::UpdateMetricsJob.unstub(:perform_later)
    Bot::UpdateMetricsJob.expects(:perform_later).with(@bot).once

    csv = generate_csv([
      ['2025-01-15 12:00:00', 'order-1', 'Market', 'Buy', '0.001', '50', '50000',
       'BTC', 'USD', 'closed'],
      ['2025-01-15 12:00:00', 'order-2', 'Market', 'Buy', '0.002', '100', '50000',
       'BTC', 'USD', 'closed']
    ])

    @bot.import_orders_csv(csv)
  end

  test 'import_orders_csv does not trigger UpdateMetricsJob when nothing imported' do
    Bot::UpdateMetricsJob.unstub(:perform_later)
    Bot::UpdateMetricsJob.expects(:perform_later).never

    csv = generate_csv([
      ['2025-01-15 12:00:00', 'order-1', 'Market', 'Buy', '0.001', '50', '50000',
       'ETH', 'EUR', 'closed']
    ])

    @bot.import_orders_csv(csv)
  end

  # == Round-trip ==

  test 'export then import preserves data' do
    create(:transaction, bot: @bot,
           external_id: 'original-1', order_type: :market_order, side: :buy,
           amount: 0.001, amount_exec: 0.001,
           quote_amount: 50, quote_amount_exec: 50,
           price: 50_000, base: 'BTC', quote: 'USD',
           external_status: :closed, status: :submitted)

    csv_content = @bot.reload.orders_csv
    other_bot = create(:dca_single_asset, user: @bot.user, exchange: @bot.exchange,
                       base_asset: @bot.base_asset, quote_asset: @bot.quote_asset)
    file = StringIO.new(csv_content)

    result = other_bot.import_orders_csv(file)

    assert result[:success]
    assert_equal 1, result[:imported_count]

    imported = other_bot.transactions.last
    assert_in_delta 0.001, imported.amount.to_f
    assert_in_delta 50, imported.quote_amount.to_f
    assert_in_delta 50_000, imported.price.to_f
    assert_equal 'BTC', imported.base
    assert_equal 'USD', imported.quote
  end

  # == Dual asset bot ==

  test 'import accepts both base assets for dual asset bot' do
    btc = Asset.find_by(symbol: 'BTC') || create(:asset, :bitcoin)
    eth = Asset.find_by(symbol: 'ETH') || create(:asset, :ethereum)
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    dual_bot = create(:dca_dual_asset, base0_asset: btc, base1_asset: eth, quote_asset: usd,
                      exchange: @bot.exchange)
    base0 = dual_bot.base0_asset.symbol
    base1 = dual_bot.base1_asset.symbol
    quote = dual_bot.quote_asset.symbol

    csv = generate_csv([
      ['2025-01-15 12:00:00', 'order-1', 'Market', 'Buy', '0.001', '50', '50000',
       base0, quote, 'closed'],
      ['2025-01-15 12:00:00', 'order-2', 'Market', 'Buy', '1', '3000', '3000',
       base1, quote, 'closed']
    ])

    result = dual_bot.import_orders_csv(csv)

    assert result[:success]
    assert_equal 2, result[:imported_count]
  end

  private

  def generate_csv(rows)
    headers = ['Timestamp', 'Order ID', 'Type', 'Side', 'Amount', 'Value', 'Price',
               'Base Asset', 'Quote Asset', 'Status']

    content = CSV.generate do |csv|
      csv << headers
      rows.each { |row| csv << row }
    end

    StringIO.new(content)
  end
end
