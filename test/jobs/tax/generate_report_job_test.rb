require 'test_helper'
require 'csv'

class Tax::GenerateReportJobTest < ActiveSupport::TestCase
  # A report that omits a whole exchange must say so on its face. Anything less lets a user file a
  # document that looks complete while an exchange contributed nothing.
  test 'a failed exchange sync banners the report with the exchange name' do
    user = create(:user)
    create(:api_key, user: user, exchange: create(:binance_exchange),
                     last_synced_at: 1.day.ago, last_sync_error: 'StandardError: API error')

    rows = generate(user, 'DE', 2020)

    assert_includes rows[1].first, 'Binance'
  end

  test 'a connected but never-synced exchange banners the report too' do
    user = create(:user)
    create(:api_key, user: user, exchange: create(:kraken_exchange), last_synced_at: nil)

    rows = generate(user, 'DE', 2021)

    assert_includes rows[1].first, 'Kraken'
  end

  test 'healthy and withdrawal-only keys stay unbannered while a failed stock venue banners' do
    user = create(:user)
    create(:api_key, user: user, exchange: create(:binance_exchange), last_synced_at: 1.day.ago)
    create(:api_key, user: user, exchange: create(:kraken_exchange), key_type: :withdrawal, last_synced_at: nil)
    # Alpaca crypto reaches this report, so a failed Alpaca sync can now hide reportable rows.
    create(:api_key, user: user, exchange: create(:alpaca_exchange),
                     last_synced_at: nil, last_sync_error: 'StandardError: API error')
    create(:api_key, user: user, exchange: create(:ibkr_exchange),
                     last_synced_at: nil, last_sync_error: 'StandardError: API error')

    rows = generate(user, 'DE', 2022)
    report_text = rows.flatten.compact.join(' ')

    assert_includes report_text, 'Alpaca'
    assert_not_includes report_text, 'Binance'
    assert_not_includes report_text, 'Kraken'
    assert_not_includes report_text, 'Interactive Brokers'
    assert_equal 3, rows.size, 'headers, the Alpaca warning, and the no-transactions line'
  end

  test 'broker scope writes a German KAP report to the broker path' do
    user = create(:user)
    exchange = create(:alpaca_exchange)
    create(:api_key, user: user, exchange: exchange, last_synced_at: 1.day.ago)
    file_path = Tax::GenerateReportJob.report_path(user.id, 'DE', 2023, 'broker')

    Tax::GenerateReportJob.perform_now(user.id, 'DE', 2023, false, 'broker')

    assert File.exist?(file_path)
    first_row = CSV.parse(File.read(file_path)).first
    assert_includes first_row, 'Anlage KAP / KAP-INV — Berechnungsgrundlage'
    assert_includes first_row, 'Alpaca'
    assert_not_includes first_row, 'alpaca'
  end

  test 'broker scope rejects unsupported countries and years' do
    user = create(:user)
    exchange = create(:alpaca_exchange)
    create(:api_key, user: user, exchange: exchange, last_synced_at: 1.day.ago)

    assert_raises(ArgumentError) do
      Tax::GenerateReportJob.perform_now(user.id, 'PL', 2024, false, 'broker')
    end
    Turbo::StreamsChannel.expects(:broadcast_replace_to).with(
      "user_#{user.id}", :tax_report,
      target: 'tax-report-progress',
      partial: 'tracker/report_failed'
    )
    assert_raises(ArgumentError) do
      Tax::GenerateReportJob.perform_now(user.id, 'DE', 2022, false, 'broker')
    end
  end

  test 'Alpaca crypto disposals enter the crypto report while Alpaca shares do not' do
    user = create(:user)
    exchange = create(:alpaca_exchange)
    api_key = create(:api_key, user: user, exchange: exchange, last_synced_at: 1.day.ago)
    create(:asset, symbol: 'AAVE', external_id: 'aave', category: 'Cryptocurrency')
    create(:asset, symbol: 'AAPL', external_id: 'AAPL.US', category: 'Stock', instrument_type: 'stock')
    buy_at = Time.utc(2024, 1, 10)
    sell_at = Time.utc(2024, 6, 1)
    HistoricalPrice.create!(asset: 'AAVE', currency: 'EUR', date: buy_at.to_date, price: 100.to_d)
    create(:account_transaction, user: user, exchange: exchange, api_key: api_key,
                                 base_currency: 'AAVE', base_amount: 1,
                                 quote_currency: nil, quote_amount: nil, transacted_at: buy_at)
    create(:account_transaction, :sell, user: user, exchange: exchange, api_key: api_key,
                                        base_currency: 'AAVE', base_amount: 1,
                                        quote_currency: 'EUR', quote_amount: 150, transacted_at: sell_at)
    create(:account_transaction, user: user, exchange: exchange, api_key: api_key,
                                 base_currency: 'AAPL', base_amount: 1,
                                 quote_currency: 'EUR', quote_amount: 100, transacted_at: buy_at)
    create(:account_transaction, :sell, user: user, exchange: exchange, api_key: api_key,
                                        base_currency: 'AAPL', base_amount: 1,
                                        quote_currency: 'EUR', quote_amount: 150, transacted_at: sell_at)

    rows = generate(user, 'DE', 2024)

    assert(rows.any? { |row| row[2] == 'AAVE' })
    assert_not(rows.any? { |row| row[2] == 'AAPL' })
  end

  test 'report ready broadcast includes the scoped download URL' do
    user = create(:user)
    exchange = create(:alpaca_exchange)
    create(:api_key, user: user, exchange: exchange, last_synced_at: 1.day.ago)
    stream = Turbo::StreamsChannel.send(:stream_name_from, ["user_#{user.id}", :tax_report])
    pubsub = ActionCable.server.pubsub
    pubsub.clear_messages(stream)

    Tax::GenerateReportJob.perform_now(user.id, 'DE', 2025, false, 'broker')

    broadcast = pubsub.broadcasts(stream).last
    assert broadcast
    assert_includes ActiveSupport::JSON.decode(broadcast), 'report_scope=broker'
  end

  # `country` reaches this builder straight from user params at four call sites (the controller
  # download, check_pending_report and three MCP tools), and the filename is served as an attachment.
  # The strip is the only thing between those params and both path traversal and a
  # Content-Disposition header injection.
  test 'a hostile country param cannot escape the report directory or the scope suffix' do
    traversal = Tax::GenerateReportJob.report_path(1, '../../../etc/passwd', 2024, '../broker')

    assert traversal.start_with?(Rails.root.join('tmp', 'tax_reports').to_s), traversal
    assert traversal.end_with?('_crypto.csv'), traversal
    assert_equal 'ETCPASSWD', Tax::GenerateReportJob.report_country('../../../etc/passwd')

    crlf = Tax::GenerateReportJob.report_path(1, "DE\r\nX-Injected: 1", 2024)

    assert_equal 1, crlf.lines.size
    assert_equal Rails.root.join('tmp', 'tax_reports', '1_DEXINJECTED_2024_crypto.csv').to_s, crlf
  end

  private

  # Parallel workers each get their own database but share tmp/, so two tests generating the same
  # (user_id, country, year) would clobber each other's file. One year per test keeps them apart.
  def generate(user, country, year)
    Tax::GenerateReportJob.perform_now(user.id, country, year)
    CSV.parse(File.read(Tax::GenerateReportJob.report_path(user.id, country, year)))
  end
end
