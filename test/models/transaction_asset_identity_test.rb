require 'test_helper'

# An order records the asset it traded, not only its symbol: a symbol can be another asset's on the same
# venue, a venue can spell an asset its own way (Kraken lists Bitcoin as XBT), and symbols get renamed. The
# ids come from the ticker the order was placed on, so they are exact; the strings stay as the snapshot shown.
class TransactionAssetIdentityTest < ActiveSupport::TestCase
  def setup
    @usd = create(:asset, :usd)
    @btc = create(:asset, :bitcoin)
    @kraken = create(:kraken_exchange)
    @xbt = create(:ticker, exchange: @kraken, base_asset: @btc, quote_asset: @usd, base_symbol: 'XBT')
    @bot = create(:dca_single_asset, user: create(:user), exchange: @kraken, base_asset: @btc, quote_asset: @usd)
  end

  test 'an order records the assets of the ticker it was placed on' do
    order = { ticker: @xbt, side: :buy, order_type: :market_order, price: 100, amount: 1, quote_amount: 100 }

    placed = @bot.create_submitted_order!(order.merge(order_id: 'p-1', status: :closed, amount_exec: 1,
                                                      quote_amount_exec: 100))
    failed = @bot.create_failed_order!(order.merge(error_messages: ['nope']))
    skipped = @bot.send(:create_skipped_order!, order)

    [placed, failed, skipped].each do |row|
      assert_equal [@btc.id, @usd.id], [row.base_asset_id, row.quote_asset_id]
      assert_equal %w[BTC USD], [row.base, row.quote], 'the strings stay the snapshot'
    end
  end

  # A signal bot goes through the same creators but says what belongs on every row in its own words
  # — it has no interval and no per-order amount — so it has to say the assets too. Its failed and
  # skipped rows are never polled, so nothing would fill them in later.
  test 'a signal bot records the assets on every kind of row' do
    bot = create(:signal_bot, :started, user: create(:user), exchange: @kraken, base_asset: @btc, quote_asset: @usd)
    order = { ticker: @xbt, side: :buy, order_type: :market_order, price: 100, amount: 1, quote_amount: 100 }

    placed = bot.persist_accepted_order!(order, 's-1')
    failed = bot.create_failed_order!(order.merge(error_messages: ['nope']))
    skipped = bot.send(:create_skipped_order!, order)

    [placed, failed, skipped].each do |row|
      assert_equal [@btc.id, @usd.id], [row.base_asset_id, row.quote_asset_id], row.status
    end
  end

  test 'an order placed through a signal bot from the API records the assets' do
    bot = create(:signal_bot, :started, user: create(:user), exchange: @kraken, base_asset: @btc, quote_asset: @usd)
    stub_ticker_ask_price(bot.ticker, price: 50_000)
    bot.exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'api-asset'))

    row = bot.execute_api_order(side: :buy, amount: 100.to_d, amount_type: :quote).transaction

    assert_equal [@btc.id, @usd.id], [row.base_asset_id, row.quote_asset_id]
  end

  test "a poll fills a row's missing ids from the order's ticker and never overwrites one" do
    row = open_row(resolve_asset_ids: false)
    row.update_with_order_data(poll(@xbt))
    assert_equal [@btc.id, @usd.id], [row.reload.base_asset_id, row.quote_asset_id]

    other = create(:asset, :ethereum)
    row.update_columns(base_asset_id: other.id)
    row.update_with_order_data(poll(@xbt))
    assert_equal other.id, row.reload.base_asset_id
  end

  test 'the assets are read by id, with nothing guessed from the symbol' do
    row = open_row
    assert_equal [@btc, @usd], [row.base_asset, row.quote_asset]

    row.update_columns(base_asset_id: nil)
    assert_nil row.reload.base_asset
  end

  test "a filled id recomputes the bot's figures" do
    row = open_row(resolve_asset_ids: false)
    Bot::UpdateMetricsJob.expects(:perform_later).with(@bot).at_least_once

    row.update!(base_asset_id: @btc.id)
  end

  test 'a sale that learns its asset on a poll is reconciled for wash-sale like one that learns its fill' do
    sale = create(:transaction, bot: @bot, exchange: @kraken, side: :sell, status: :submitted, external_status: :closed,
                                external_id: 's-1', base: 'BTC', quote: 'USD', price: 100, amount: 1, amount_exec: 1,
                                quote_amount_exec: 100, resolve_asset_ids: false)
    Transaction.any_instance.expects(:reconcile_wash_sale).once

    sale.update!(base_asset_id: @btc.id)
  end

  test "a new order's page update needs no asset it cannot read" do
    row = open_row(resolve_asset_ids: false)

    assert_nothing_raised { @bot.broadcast_new_order(row) }
    assert_nothing_raised { @bot.broadcast_updated_order(row) }
  end

  private

  def open_row(**attributes)
    create(:transaction, bot: @bot, exchange: @kraken, status: :submitted, external_status: :open,
                         external_id: "o-#{SecureRandom.hex(3)}", base: 'BTC', quote: 'USD', price: 100, amount: 1,
                         amount_exec: nil, quote_amount_exec: nil, **attributes)
  end

  def poll(ticker)
    { ticker:, status: :open, price: 100, amount: 1, quote_amount: 100, side: :buy, order_type: :market_order,
      amount_exec: nil, quote_amount_exec: nil }
  end
end
