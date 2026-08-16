require 'test_helper'

class TrackerControllerTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    create(:user, admin: true, setup_completed: true) # platform requires an admin to exist
    @user = create(:user, setup_completed: true)
    @api_key = create(:api_key, user: @user)
    @transacted_at = Time.zone.parse('2026-08-01 12:00:00')
    @report_paths = []
    self.default_url_options = { locale: nil }
    sign_in @user
  end

  teardown do
    @report_paths.each { |path| FileUtils.rm_f(path) }
  end

  test 'links a withdrawal to its only eligible deposit' do
    withdrawal = create_transaction(:withdrawal, base_amount: 1, transacted_at: @transacted_at)
    deposit = create_transaction(:deposit, base_amount: 0.999, transacted_at: @transacted_at + 2.days)

    patch toggle_transfer_tracker_transaction_path(withdrawal)

    assert_response :success
    assert_equal deposit.id, withdrawal.reload.linked_transaction_id
    # A wrong dom_id target is a silent no-op in the browser, so pin both rows of the pair.
    assert_match(/target="account_transaction_#{withdrawal.id}"/, @response.body)
    assert_match(/target="account_transaction_#{deposit.id}"/, @response.body)
  end

  test 'unlinks a linked pair from the withdrawal' do
    withdrawal, = create_linked_pair

    patch toggle_transfer_tracker_transaction_path(withdrawal)

    assert_response :success
    assert_nil withdrawal.reload.linked_transaction_id
  end

  test 'unlinking sticks: the matcher must not re-link it on the next sync' do
    withdrawal, = create_linked_pair

    patch toggle_transfer_tracker_transaction_path(withdrawal)
    TransferMatcher.run!(@user)

    assert_nil withdrawal.reload.linked_transaction_id
    assert_predicate withdrawal, :transfer_link_rejected?
  end

  test 'linking again clears the rejection so the matcher can maintain the pair' do
    withdrawal = create_transaction(:withdrawal, base_amount: 1, transacted_at: @transacted_at)
    deposit = create_transaction(:deposit, base_amount: 0.999, transacted_at: @transacted_at + 2.days)
    withdrawal.update!(transfer_link_rejected: true)

    patch toggle_transfer_tracker_transaction_path(withdrawal)

    assert_equal deposit.id, withdrawal.reload.linked_transaction_id
    assert_not_predicate withdrawal, :transfer_link_rejected?
  end

  test 'unlinks a linked pair from the deposit' do
    withdrawal, deposit = create_linked_pair

    patch toggle_transfer_tracker_transaction_path(deposit)

    assert_response :success
    assert_nil withdrawal.reload.linked_transaction_id
  end

  test 'does not link a withdrawal with no candidate' do
    withdrawal = create_transaction(:withdrawal, base_amount: 1, transacted_at: @transacted_at)

    patch toggle_transfer_tracker_transaction_path(withdrawal)

    assert_response :success
    assert_nil withdrawal.reload.linked_transaction_id
    assert flash[:alert].present?
  end

  test 'does not guess between multiple candidates' do
    withdrawal = create_transaction(:withdrawal, base_amount: 1, transacted_at: @transacted_at)
    create_transaction(:deposit, base_amount: 0.999, transacted_at: @transacted_at + 1.day)
    create_transaction(:deposit, base_amount: 0.998, transacted_at: @transacted_at + 2.days)

    patch toggle_transfer_tracker_transaction_path(withdrawal)

    assert_response :success
    assert_nil withdrawal.reload.linked_transaction_id
    assert flash[:alert].present?
  end

  test "does not find another user's transaction" do
    show_exceptions = Rails.application.env_config['action_dispatch.show_exceptions']
    other_user = create(:user, setup_completed: true)
    other_api_key = create(:api_key, user: other_user, exchange: @api_key.exchange)
    withdrawal = create(
      :account_transaction, :withdrawal,
      api_key: other_api_key, transacted_at: @transacted_at
    )
    Rails.application.env_config['action_dispatch.show_exceptions'] = :none

    assert_raises(ActiveRecord::RecordNotFound) do
      patch toggle_transfer_tracker_transaction_path(withdrawal)
    end
    assert_nil withdrawal.reload.linked_transaction_id
  ensure
    Rails.application.env_config['action_dispatch.show_exceptions'] = show_exceptions
  end

  test 'links a deposit to its only eligible earlier withdrawal' do
    withdrawal = create_transaction(:withdrawal, base_amount: 1, transacted_at: @transacted_at)
    deposit = create_transaction(:deposit, base_amount: 0.999, transacted_at: @transacted_at + 2.days)

    patch toggle_transfer_tracker_transaction_path(deposit)

    assert_response :success
    assert_equal deposit.id, withdrawal.reload.linked_transaction_id
  end

  test 'does not consider a deposit larger than the withdrawal' do
    withdrawal = create_transaction(:withdrawal, base_amount: 1, transacted_at: @transacted_at)
    create_transaction(:deposit, base_amount: 1.5, transacted_at: @transacted_at + 2.days)

    patch toggle_transfer_tracker_transaction_path(withdrawal)

    assert_response :success
    assert_nil withdrawal.reload.linked_transaction_id
    assert flash[:alert].present?
  end

  # The turbo-stream tests above render the row partial in isolation. This is the only check that
  # the transactions table itself still renders once the badge and the toggle button are in it.
  test 'the transactions table shows the transfer badge and the toggle action' do
    withdrawal, = create_linked_pair

    get tracker_path

    assert_response :success
    assert_no_match(/translation_missing/, @response.body)
    assert_includes @response.body, I18n.t('tracker.transfer_badge')
    assert_includes @response.body, I18n.t('tracker.transfer_unlink')
    assert_includes @response.body, toggle_transfer_tracker_transaction_path(withdrawal)
  end

  test 'export modal hides the broker report without an Alpaca ledger' do
    MarketData.stubs(:configured?).returns(true)

    get export_modal_tracker_path

    assert_response :success
    assert_not_includes @response.body, 'value="broker_tax_report"'
  end

  test 'export modal shows the broker classification panel with exact ledger symbols' do
    MarketData.stubs(:configured?).returns(true)
    alpaca = create(:alpaca_exchange)
    alpaca_key = create(:api_key, user: @user, exchange: alpaca)
    create(:asset, symbol: 'Brk.B', category: 'Stock', instrument_type: 'stock')
    create(
      :account_transaction,
      user: @user,
      api_key: alpaca_key,
      exchange: alpaca,
      base_currency: 'Brk.B',
      transacted_at: Time.utc(2024, 3, 1)
    )

    get export_modal_tracker_path

    assert_response :success
    assert_includes @response.body, 'value="broker_tax_report"'
    assert_includes @response.body, 'data-tracker-export-target="classificationPanel"'
    assert_includes @response.body, 'data-symbol="Brk.B"'
    assert_no_match(/translation_missing/, @response.body)
  end

  # A hundred holdings used to render a hundred rows, and the user had to scroll all of them to
  # discover that none needed a decision. Only the rows that ask something render up front.
  test 'the classification panel renders only the rows that need a decision' do
    MarketData.stubs(:configured?).returns(true)
    broker_ledger('AAPL' => 'stock', 'VT' => 'etf', 'MYSTERY' => nil)

    get export_modal_tracker_path

    assert_response :success
    pending, settled = classification_rows_by_visibility
    # Unclassified first — it blocks the report — then the fund we guessed at 0% Teilfreistellung.
    assert_equal %w[MYSTERY VT], symbols_of(pending)
    assert_equal %w[AAPL], symbols_of(settled)
    assert_no_match(/translation_missing/, @response.body)
  end

  test 'a portfolio of nothing but shares asks for nothing and folds into the disclosure' do
    MarketData.stubs(:configured?).returns(true)
    broker_ledger('AAPL' => 'stock', 'MSFT' => 'stock')

    get export_modal_tracker_path

    assert_response :success
    pending, settled = classification_rows_by_visibility
    assert_empty pending
    assert_equal %w[AAPL MSFT], symbols_of(settled)
    assert_includes @response.body, I18n.t('tracker.export_modal.classification_settled', count: 2)
  end

  # The disclosure stays the only place a classification can be changed, so its selects — including
  # the Type one, the only route to `other_security` — must still be there.
  test 'a fund the user has already decided sits in the disclosure, still editable' do
    MarketData.stubs(:configured?).returns(true)
    broker_ledger('VT' => 'etf')
    FundClassification.create!(user: @user, symbol: 'VT', kind: :fund, fund_category: :equity_fund)

    get export_modal_tracker_path

    assert_response :success
    pending, settled = classification_rows_by_visibility
    assert_empty pending
    row = settled.sole
    assert row.at_css('select[data-role="kind"] option[value="other_security"]')
    assert_equal 'equity_fund', row.at_css('select[data-role="category"] option[selected]')['value']
  end

  # Both values on this card are our guess from the market-data catalogue, so both stay changeable.
  # Re-typing a mis-catalogued ETN must not cost a save of `kind=fund` first — that would persist the
  # §20(4) election the taxpayer never made.
  test 'a proposed fund offers the type as well as the category' do
    MarketData.stubs(:configured?).returns(true)
    broker_ledger('VT' => 'etf')

    get export_modal_tracker_path

    assert_response :success
    pending, = classification_rows_by_visibility
    row = pending.sole
    assert_equal 'fund', row.at_css('select[data-role="kind"] option[selected]')['value']
    assert row.at_css('select[data-role="kind"] option[value="other_security"]')
    assert_equal 'other_fund', row.at_css('select[data-role="category"] option[selected]')['value']
  end

  # Nothing left to decide: the user already stated the type, and the refusal is not theirs to lift.
  test 'a refused row the user has decided is read-only and names its refusal reason' do
    MarketData.stubs(:configured?).returns(true)
    broker_ledger('AAPL' => 'stock')
    stub_classification_rows(
      classification_row(symbol: 'AAPL', kind: :share, persisted: true,
                         refusal_reasons: %i[unsupported_activity])
    )

    get export_modal_tracker_path

    assert_response :success
    pending, = classification_rows_by_visibility
    row = pending.sole
    assert_empty row.css('select')
    assert_equal 'share', row.at_css('input[type="hidden"][data-role="kind"]')['value']
    # Both halves ride along, or the gate reads `null.value` off the row and kills Generate.
    assert_equal '', row.at_css('input[type="hidden"][data-role="category"]')['value']
    assert_includes row.text,
                    I18n.t('tracker.export_modal.classification_refusal_reasons.unsupported_activity')
    assert_no_match(/translation_missing/, @response.body)
  end

  # A refusal withholds this year's figures; it does not withdraw the user's right to state the
  # category. Locking the row at our pessimistic 0% default would show them the expensive answer and
  # deny them the correction — and the classification outlives this report.
  test 'a refused fund we proposed keeps both selects' do
    MarketData.stubs(:configured?).returns(true)
    broker_ledger('VT' => 'etf')
    stub_classification_rows(
      classification_row(symbol: 'VT', kind: :fund, fund_category: :other_fund,
                         refusal_reasons: %i[pre_2018_fund_lot])
    )

    get export_modal_tracker_path

    assert_response :success
    pending, = classification_rows_by_visibility
    row = pending.sole
    assert_equal 'other_fund', row.at_css('select[data-role="category"] option[selected]')['value']
    assert row.at_css('select[data-role="kind"]')
    assert_includes row.text, I18n.t('tracker.export_modal.classification_fund_hint')
    assert_includes row.text,
                    I18n.t('tracker.export_modal.classification_refusal_reasons.pre_2018_fund_lot')
  end

  # A symbol can be refused and unclassified at once. Unclassified wins: it is the one that still
  # needs an answer, and a row rendered twice would let two controls disagree about one symbol.
  test 'a row that is both unclassified and refused renders once, asking for the type' do
    MarketData.stubs(:configured?).returns(true)
    broker_ledger('AAPL' => 'stock')
    stub_classification_rows(
      classification_row(symbol: 'ZZZ', kind: nil, refusal_reasons: %i[missing_price])
    )

    get export_modal_tracker_path

    assert_response :success
    pending, settled = classification_rows_by_visibility
    assert_empty settled
    row = pending.sole
    assert_equal 'ZZZ', row['data-symbol']
    assert row.at_css('select[data-role="kind"]')
  end

  # `classificationsComplete` reads `[data-role="kind"]` and `[data-role="category"]` on every row
  # it finds. A row rendered without one is a TypeError that takes out the Generate button, in the
  # browser, where nothing else in this suite would see it.
  test 'every rendered row carries the two controls the completeness gate reads' do
    MarketData.stubs(:configured?).returns(true)
    broker_ledger('AAPL' => 'stock', 'VT' => 'etf', 'MYSTERY' => nil)
    FundClassification.create!(user: @user, symbol: 'AAPL', kind: :share)

    get export_modal_tracker_path

    assert_response :success
    rows = classification_rows_by_visibility.flatten
    assert_equal 3, rows.size
    rows.each do |row|
      kind = row.at_css('[data-role="kind"]')
      category = row.at_css('[data-role="category"]')
      assert kind, "#{row['data-symbol']} has no kind control"
      assert category, "#{row['data-symbol']} has no category control"
      # Mirrors the gate: a fund is complete only with a category behind it.
      value = kind.name == 'select' ? kind.at_css('option[selected]')&.[]('value') : kind['value']
      next unless value == 'fund'

      selected = category.at_css('option[selected]')&.[]('value') || category['value']
      assert selected.present?, "#{row['data-symbol']} is a fund with no category"
    end
  end

  # The disclaimer is about the tax report; it sat outside the toggled block and so was shown over
  # the plain transactions export too.
  test 'the tax disclaimer is inside the tax-only options block' do
    MarketData.stubs(:configured?).returns(true)

    get export_modal_tracker_path

    assert_response :success
    disclaimer = Nokogiri::HTML(@response.body).at_css("p:contains('#{I18n.t('tracker.export_modal.tax_disclaimer')}')")
    assert disclaimer, 'the disclaimer paragraph is missing'
    assert_equal 'taxOptions', disclaimer['data-tracker-export-target']
  end

  # A sync failure is persisted on the key but the banner was only ever broadcast, so a user who
  # reloaded the page saw a tracker that looked healthy while an exchange contributed nothing.
  test 'a persisted sync failure banners on page load' do
    @api_key.update_column(:last_sync_error, 'StandardError: API error')

    get tracker_path

    assert_response :success
    assert_includes @response.body, I18n.t('tracker.sync_failed_for', exchanges: @api_key.exchange.name)
  end

  test 'a healthy never-synced key does not banner a sync failure' do
    @api_key.update!(last_synced_at: nil, last_sync_error: nil)

    get tracker_path

    assert_response :success
    assert_not_includes @response.body, I18n.t('tracker.sync_failed_for', exchanges: @api_key.exchange.name)
  end

  test 'creates fund classifications for the signed-in user' do
    patch fund_classifications_tracker_path, params: {
      classifications: [
        { symbol: 'AAPL', kind: 'share' },
        { symbol: 'VT', kind: 'fund', fund_category: 'other_fund' }
      ]
    }, as: :json

    assert_response :success
    assert_equal 'share', @user.fund_classifications.find_by!(symbol: 'AAPL').kind
    fund = @user.fund_classifications.find_by!(symbol: 'VT')
    assert_equal 'fund', fund.kind
    assert_equal 'other_fund', fund.fund_category
  end

  test 'stores a fund classification symbol exactly as given' do
    patch fund_classifications_tracker_path, params: {
      classifications: [{ symbol: 'Brk.B', kind: 'share' }]
    }, as: :json

    assert_response :success
    # The resolver uses the ledger's exact spelling, so normalising this value would make it miss.
    assert_equal 'Brk.B', @user.fund_classifications.pick(:symbol)
  end

  test "does not update another user's fund classification" do
    other_user = create(:user, setup_completed: true)
    other = FundClassification.create!(user: other_user, symbol: 'AAPL', kind: :share)

    patch fund_classifications_tracker_path, params: {
      classifications: [{ symbol: 'AAPL', kind: 'fund', fund_category: 'other_fund' }]
    }, as: :json

    assert_response :success
    assert_equal 'share', other.reload.kind
    assert_nil other.fund_category
    own = @user.fund_classifications.find_by!(symbol: 'AAPL')
    assert_equal 'fund', own.kind
    assert_equal 'other_fund', own.fund_category
  end

  test 'updating a fund classification clears a stale fund category' do
    classification = FundClassification.create!(
      user: @user, symbol: 'VT', kind: :fund, fund_category: :equity_fund
    )

    patch fund_classifications_tracker_path, params: {
      classifications: [{ symbol: 'VT', kind: 'share' }]
    }, as: :json

    assert_response :success
    assert_equal 'share', classification.reload.kind
    assert_nil classification.fund_category
  end

  test 'reports a fund classification that failed validation instead of a silent ok' do
    assert_no_difference('FundClassification.count') do
      patch fund_classifications_tracker_path, params: {
        classifications: [{ symbol: 'VT', kind: 'fund' }] # fund with no category — invalid
      }, as: :json
    end

    assert_response :unprocessable_entity
  end

  # A hand-crafted request can send `classifications` as a numeric-keyed hash, which `permit`
  # keeps as Parameters whose `each` yields [key, value] pairs — indexing those used to 500.
  test 'ignores a malformed classifications shape' do
    assert_no_difference('FundClassification.count') do
      patch fund_classifications_tracker_path,
            params: { classifications: { '0' => { symbol: 'AAPL', kind: 'share' } } }
    end

    assert_response :success
  end

  test 'skips an unknown fund classification kind' do
    assert_no_difference('FundClassification.count') do
      patch fund_classifications_tracker_path, params: {
        classifications: [{ symbol: 'AAPL', kind: 'unknown' }]
      }, as: :json
    end

    assert_response :success
  end

  # tmp/tax_reports is shared by every parallel worker, user ids repeat across their databases, and the
  # runner splits tests rather than files — so each of these fixtures needs a year of its own.
  test 'downloads the broker report with the broker filename' do
    write_report(country: 'DE', year: 1991, report_scope: 'broker', contents: 'broker report')

    get download_tax_report_tracker_path(country: 'DE', year: 1991, report_scope: 'broker')

    assert_response :success
    assert_equal 'broker report', response.body
    assert_match(/filename="deltabadger-broker-tax-report-de-1991\.csv"/,
                 response.headers['Content-Disposition'])
  end

  test 'downloads the default crypto report with the existing filename' do
    write_report(country: 'DE', year: 1992, contents: 'crypto report')

    get download_tax_report_tracker_path(country: 'DE', year: 1992)

    assert_response :success
    assert_equal 'crypto report', response.body
    assert_match(/filename="deltabadger-tax-report-de-1992\.csv"/,
                 response.headers['Content-Disposition'])
  end

  test 'index auto-downloads a pending broker report with its scope' do
    @user.update!(tracker_settings: {
                    'pending_report' => { 'country' => 'DE', 'year' => 1993, 'report_scope' => 'broker' }
                  })
    write_report(country: 'DE', year: 1993, report_scope: 'broker', contents: 'broker report')

    get tracker_path

    assert_response :success
    assert_includes response.body, 'report_scope=broker'
  end

  # A broker run is always DE. Writing that into the shared preference keys left the crypto form
  # pre-selected on Germany, so the next Generate silently filed the wrong jurisdiction.
  test 'generating a broker report leaves the crypto export preferences alone' do
    Tax::GenerateReportJob.stubs(:perform_later)
    @user.update!(tracker_settings: { 'export_type' => 'tax_report', 'country' => 'FR', 'year' => 2026 })

    get tax_report_tracker_path(country: 'DE', year: 2024, report_scope: 'broker')

    assert_response :success
    settings = @user.reload.tracker_settings
    assert_equal 'FR', settings['country']
    assert_equal 2026, settings['year']
  end

  # Toggling the modal's radio rewrites preferences. A crypto report already generating must still
  # be found afterwards, or the finished file never auto-downloads and the user is told nothing.
  test 'switching the modal to the broker scope does not lose an in-flight crypto report' do
    Tax::GenerateReportJob.stubs(:perform_later)
    get tax_report_tracker_path(country: 'FR', year: 1989)
    write_report(country: 'FR', year: 1989, contents: 'crypto report')

    patch save_export_settings_tracker_path, params: { export_type: 'tax_report', report_scope: 'broker' }
    get tracker_path

    assert_response :success
    assert_includes CGI.unescapeHTML(response.body),
                    download_tax_report_tracker_path(country: 'FR', year: 1989, report_scope: 'crypto')
  end

  test 'tax report persists and enqueues the broker scope' do
    base_adapter = ActiveJob::Base.queue_adapter
    job_adapter = Tax::GenerateReportJob.queue_adapter
    test_adapter = ActiveJob::QueueAdapters::TestAdapter.new
    ActiveJob::Base.queue_adapter = test_adapter
    Tax::GenerateReportJob.queue_adapter = test_adapter

    assert_enqueued_with(
      job: Tax::GenerateReportJob,
      args: [@user.id, 'DE', 2024, false, 'broker']
    ) do
      get tax_report_tracker_path(country: 'DE', year: 2024, report_scope: 'broker')
    end

    assert_response :success
    assert_equal({ 'country' => 'DE', 'year' => 2024, 'report_scope' => 'broker' },
                 @user.reload.tracker_settings['pending_report'])
  ensure
    Tax::GenerateReportJob.queue_adapter = job_adapter
    ActiveJob::Base.queue_adapter = base_adapter
  end

  private

  def write_report(country:, year:, contents:, report_scope: 'crypto')
    path = Tax::GenerateReportJob.report_path(@user.id, country, year, report_scope)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, contents)
    @report_paths << path
    path
  end

  def create_transaction(entry_type, **attributes)
    create(:account_transaction, entry_type, api_key: @api_key, **attributes)
  end

  def create_linked_pair
    withdrawal = create_transaction(:withdrawal, base_amount: 1, transacted_at: @transacted_at)
    deposit = create_transaction(:deposit, base_amount: 0.999, transacted_at: @transacted_at + 2.days)
    withdrawal.update!(linked_transaction: deposit)
    [withdrawal, deposit]
  end

  # An Alpaca ledger holding each symbol once. `instrument_type` is what FundClassification.resolve
  # proposes from: 'stock' → share, 'etf' → fund/other_fund, nil → no asset row at all, which is
  # what an unclassified symbol looks like.
  def broker_ledger(symbols)
    alpaca = Exchanges::Alpaca.first || create(:alpaca_exchange)
    key = @user.api_keys.find_by(exchange: alpaca) || create(:api_key, user: @user, exchange: alpaca)

    symbols.each do |symbol, instrument_type|
      create(:asset, symbol: symbol, category: 'Stock', instrument_type: instrument_type) if instrument_type
      create(:account_transaction, user: @user, api_key: key, exchange: alpaca,
                                   base_currency: symbol, transacted_at: Time.utc(2024, 3, 1))
    end
  end

  # [rows the panel renders directly, rows folded into the <details> disclosure].
  def classification_rows_by_visibility
    Nokogiri::HTML(@response.body)
            .css('[data-tracker-export-target="classificationRow"]')
            .partition { |row| row.ancestors('details').empty? }
  end

  def symbols_of(rows)
    rows.map { |row| row['data-symbol'] }
  end

  def stub_classification_rows(*rows)
    Tax::BrokerReport.any_instance.stubs(:classification_rows).returns(rows)
  end

  def classification_row(symbol:, kind:, fund_category: nil, persisted: false, refusal_reasons: [])
    { symbol: symbol, kind: kind, fund_category: fund_category, classified: !kind.nil?,
      persisted: persisted, refused: refusal_reasons.any?, refusal_reasons: refusal_reasons }
  end
end
