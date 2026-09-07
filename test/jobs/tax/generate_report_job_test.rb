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

  # A status check or a download landing mid-write must see a whole report or none, so the CSV is
  # written beside the final path and renamed into place.
  test 'the report is published atomically, leaving no partial file behind' do
    user = create(:user)
    path = Tax::GenerateReportJob.report_path(user.id, 'DE', 2026)

    Tax::GenerateReportJob.perform_now(user.id, 'DE', 2026)

    assert File.exist?(path)
    assert_not File.exist?("#{path}.tmp")
  ensure
    FileUtils.rm_f(path)
  end

  private

  # Parallel workers each get their own database but share tmp/, so two tests generating the same
  # (user_id, country, year) would clobber each other's file. One year per test keeps them apart.
  # THE load-bearing test. crypto_transactions includes every non-stock-venue transaction
  # unconditionally — CryptoScope is consulted only for stock venues — so before this a Kraken
  # xStock bought and sold more than a year apart silently took the crypto holding exemption.
  #
  # Each of these owns a distinct year: reports are real files under a shared tmp root, keyed on user
  # id, which repeats across parallel workers. Sharing a year would let one worker's refusal decide
  # another's report.
  test 'refuses a crypto report when a tokenized asset was traded on a crypto venue' do
    user = tokenized_holder(2015)

    Tax::GenerateReportJob.perform_now(user.id, 'DE', 2015)

    assert_not File.exist?(Tax::GenerateReportJob.report_path(user.id, 'DE', 2015)),
               'no report may be produced for an instrument we cannot classify'
    refusal = Tax::GenerateReportJob.refusal(user.id, 'DE', 2015)
    assert_equal 'tokenized_unsupported', refusal['reason']
    assert_equal ['NVDAX'], refusal['symbols']
  end

  test 'a report with no tokenized activity is still generated' do
    user = create(:user)
    create(:api_key, user: user, exchange: create(:kraken_exchange), last_synced_at: 1.day.ago)

    Tax::GenerateReportJob.perform_now(user.id, 'DE', 2016)

    assert File.exist?(Tax::GenerateReportJob.report_path(user.id, 'DE', 2016))
    assert_nil Tax::GenerateReportJob.refusal(user.id, 'DE', 2016)
  end

  # A refusal that outlives its cause is the same bug as the stale CSV it replaces.
  test 'a successful regeneration clears an earlier refusal' do
    user = tokenized_holder(2017)
    Tax::GenerateReportJob.perform_now(user.id, 'DE', 2017)
    assert Tax::GenerateReportJob.refusal(user.id, 'DE', 2017), 'precondition: refused'

    AccountTransaction.where(user: user, base_currency: 'NVDAX').delete_all
    Tax::GenerateReportJob.perform_now(user.id, 'DE', 2017)

    assert_nil Tax::GenerateReportJob.refusal(user.id, 'DE', 2017)
    assert File.exist?(Tax::GenerateReportJob.report_path(user.id, 'DE', 2017))
  end

  # Refusing must not leave the earlier, wrongly-exempted CSV downloadable.
  test 'refusing removes a previously generated report' do
    user = tokenized_holder(2018)
    path = Tax::GenerateReportJob.report_path(user.id, 'DE', 2018)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, "stale\n")

    Tax::GenerateReportJob.perform_now(user.id, 'DE', 2018)

    assert_not File.exist?(path)
  end

  def tokenized_holder(year)
    user = create(:user)
    exchange = create(:kraken_exchange)
    create(:api_key, user: user, exchange: exchange, last_synced_at: 1.day.ago)
    create(:asset, symbol: 'NVDAX', name: 'NVIDIA xStock',
                   external_id: 'nvidia-xstock', instrument_type: 'tokenized')
    create(:account_transaction, user: user, exchange: exchange, base_currency: 'NVDAX',
                                 entry_type: :buy, transacted_at: Time.utc(year, 3, 1))
    @cleanup = [user.id, year]
    user
  end

  def teardown
    return unless defined?(@cleanup) && @cleanup

    user_id, year = @cleanup
    FileUtils.rm_f(Tax::GenerateReportJob.report_path(user_id, 'DE', year))
    FileUtils.rm_f(Tax::GenerateReportJob.refusal_path(user_id, 'DE', year))
  end

  def generate(user, country, year)
    Tax::GenerateReportJob.perform_now(user.id, country, year)
    CSV.parse(File.read(Tax::GenerateReportJob.report_path(user.id, country, year)))
  end
end
