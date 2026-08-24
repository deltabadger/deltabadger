class TrackerController < ApplicationController
  include AdminOnly

  # Changing this also means editing the 15 tracker.transfer_no_candidate translations, which spell
  # "14 days" out rather than interpolating it.
  TRANSFER_LINK_WINDOW = 14.days

  # The table shows a window, not an archive: an account with five years of Binance fills renders
  # tens of thousands of rows otherwise. `?all=1` still asks for the lot, and Export always has it.
  ROW_LIMIT = 200

  before_action :authenticate_user!
  before_action :require_admin!, only: :setup_coingecko

  def index
    @scope_exchange = Exchange.find(params[:exchange_id]) if params[:exchange_id].present?
    exchange_ids = (current_user.api_keys.pluck(:exchange_id) +
      current_user.account_transactions.distinct.pluck(:exchange_id)).uniq
    @exchanges = Exchange.where(id: exchange_ids).order(:name)
    @exchanges_with_valid_keys = current_user.api_keys.where(key_type: :trading, status: :correct).pluck(:exchange_id).to_set
    @has_syncable_keys = @exchanges_with_valid_keys.any?
    @addable_exchanges = Exchange.available.where.not(id: exchange_ids).order(:name)
    user_transactions = AccountTransaction.for_user(current_user)
    @date_from = params[:from].presence || user_transactions.minimum(:transacted_at)&.to_date&.iso8601
    @date_to = params[:to].presence || Date.current.iso8601
    rows = filtered_transactions.by_date.includes(:exchange, :bot_transaction, :linked_transaction, :inverse_link)
    @account_transactions = params[:all].present? ? rows : rows.limit(ROW_LIMIT)
    # A sync failure survives the page it was broadcast onto, so the banner has to be rebuilt on
    # load — otherwise a persisted failure is invisible until the next sync. Only `:failed`: a
    # never-synced key is `sync_issue`'s other reason and is not a failure to shout about here.
    @sync_failures = current_user.api_keys.includes(:exchange).filter_map do |api_key|
      issue = api_key.sync_issue
      issue[:exchange] if issue && issue[:reason] == :failed
    end
    load_portfolio
    load_ledgers
    load_history
    check_pending_report
  end

  def sync
    api_keys = current_user.api_keys.where(key_type: :trading, status: :correct).includes(:exchange)
    return head :no_content if api_keys.empty?

    AccountTransaction::SyncTrackerJob.perform_later(current_user.id, api_keys.map(&:id))
    AccountBalance::SyncJob.perform_later(current_user.id, api_keys.map(&:id))

    exchange_names = api_keys.map { |k| k.exchange.name }.join(', ')
    render turbo_stream: turbo_stream.append(
      'flash', partial: 'tracker/sync_progress', locals: { exchange_name: exchange_names }
    )
  end

  def toggle_transfer
    transaction = AccountTransaction.for_user(current_user).find(params[:id])

    if transaction.linked?
      withdrawal = transaction.withdrawal? ? transaction : transaction.inverse_link
      partner = withdrawal.linked_transaction
      # Sticky, or TransferMatcher re-links the pair on the next sync and the undo silently undoes itself.
      withdrawal.update!(linked_transaction_id: nil, transfer_link_rejected: true)
      flash.now[:notice] = t('tracker.transfer_unlinked')
      rows = [withdrawal, partner]
    else
      candidates = transfer_candidates(transaction)
      if candidates.size == 1
        withdrawal, deposit = transaction.withdrawal? ? [transaction, candidates.first] : [candidates.first, transaction]
        withdrawal.update!(linked_transaction_id: deposit.id, transfer_link_rejected: false)
        flash.now[:notice] = t('tracker.transfer_linked')
        rows = [withdrawal, deposit]
      else
        flash.now[:alert] = t('tracker.transfer_no_candidate')
        rows = [transaction]
      end
    end

    # Linking a transfer changes whether the coins ever left the portfolio, and the snapshots that
    # said they did are now wrong. Coverage cannot notice — the dates did not move — so the one
    # action that edits history asks for the rebuild itself.
    PortfolioSnapshot::BackfillJob.perform_later(current_user.id)
    streams = rows.compact.map do |row|
      turbo_stream.replace(
        helpers.dom_id(row),
        partial: 'tracker/transaction_row',
        locals: { at: row.reload }
      )
    end
    render turbo_stream: streams + [turbo_stream_prepend_flash]
  end

  def export
    current_user.update(tracker_settings: (current_user.tracker_settings || {}).merge('export_type' => 'transactions'))

    transactions = filtered_transactions
    csv_data = AccountTransaction.to_csv(transactions)
    filename = "deltabadger-transactions-#{Date.current.iso8601}.csv"
    send_data csv_data, filename: filename, type: 'text/csv; charset=utf-8'
  end

  def export_modal
    unless MarketData.configured?
      render :setup_coingecko, layout: false
      return
    end

    @settings = current_user.tracker_settings || {}
    @jurisdictions = Tax::Jurisdictions.available
    user_transactions = AccountTransaction.for_user(current_user)
    @earliest_date = user_transactions.minimum(:transacted_at)&.to_date&.iso8601
    @latest_date = user_transactions.maximum(:transacted_at)&.to_date&.iso8601 || Date.current.iso8601
    # The panel classifies against the widest supported year: `Tax::BrokerReport`'s universe is
    # cumulative (everything before the year ends), so the newest supported year is a superset of
    # every earlier one — one render covers whichever year the user then picks.
    @broker_exchange = Tax::GenerateReportJob.broker_exchange(current_user)
    @fund_classification_rows = if @broker_exchange
                                  Tax::BrokerReport.new(user: current_user,
                                                        year: Tax::BrokerReport::SUPPORTED_YEARS.max,
                                                        exchange: @broker_exchange).classification_rows
                                else
                                  []
                                end
    render layout: false
  end

  def setup_coingecko
    api_key = params[:api_key]

    unless validate_coingecko_api_key(api_key)
      flash.now[:alert] = t('setup.invalid_coingecko_api_key')
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
      return
    end

    AppConfig.coingecko_api_key = api_key
    AppConfig.market_data_provider = MarketDataSettings::PROVIDER_COINGECKO
    Setup::SeedAndSyncJob.perform_later

    render turbo_stream: turbo_stream_redirect(tracker_path)
  end

  def save_export_settings
    settings = (current_user.tracker_settings || {}).merge(
      params.permit(:export_type, :country, :year, :report_scope).to_h.compact_blank
    )
    current_user.update(tracker_settings: settings)
    head :ok
  end

  def fund_classifications
    # Anything but a list of rows is a hand-crafted request, not the panel: a numeric-keyed hash
    # survives `permit` as Parameters, whose `each` yields [key, value] pairs rather than rows.
    rows = params.permit(classifications: %i[symbol kind fund_category])[:classifications]
    rows = [] unless rows.is_a?(Array)
    # `reject`, not `all?` — a failing row must not stop the rows after it from being written.
    failures = rows.reject { |row| upsert_fund_classification(row) }
    head failures.any? ? :unprocessable_entity : :ok
  end

  def tax_report
    country = params[:country]
    year = params[:year].to_i
    report_scope = Tax::GenerateReportJob::SCOPES.include?(params[:report_scope]) ? params[:report_scope] : 'crypto'
    jurisdiction = Tax::Jurisdictions.for(country)

    unless jurisdiction
      redirect_to tracker_path, alert: t('tracker.tax_report.invalid_country')
      return
    end

    # Only the pending report's identity, never the export preferences. The sibling `country`,
    # `year` and `report_scope` keys are what the user picked in the crypto form; a broker run
    # writing DE into them left the next crypto report pre-set to the wrong jurisdiction.
    current_user.update(tracker_settings: (current_user.tracker_settings || {}).merge(
      'pending_report' => { 'country' => country, 'year' => year, 'report_scope' => report_scope }
    ))

    stablecoin_as_fiat = params[:stablecoin_as_fiat] == 'true'
    Tax::GenerateReportJob.perform_later(current_user.id, country, year, stablecoin_as_fiat, report_scope)

    render turbo_stream: turbo_stream.append('flash', partial: 'tracker/report_progress')
  end

  def download_tax_report
    country = params[:country]
    year = params[:year].to_i
    report_scope = params[:report_scope]
    file_path = Tax::GenerateReportJob.report_path(current_user.id, country, year, report_scope)

    if File.exist?(file_path)
      csv_data = File.read(file_path)
      File.delete(file_path)
      report_name = report_scope == 'broker' ? 'broker-tax-report' : 'tax-report'
      report_country = Tax::GenerateReportJob.report_country(params[:country]).downcase
      filename = "deltabadger-#{report_name}-#{report_country}-#{year}.csv"
      send_data csv_data, filename: filename, type: 'text/csv; charset=utf-8'
    else
      redirect_to tracker_path, alert: t('tracker.tax_report.expired')
    end
  end

  private

  # True when there was nothing to write or the write succeeded. A row we cannot interpret (wrong
  # shape from a hand-crafted request, blank symbol, unknown kind) is skipped — but a row we DID
  # try to write and failed validation on must not be reported back as :ok, or the endpoint claims
  # a write it never performed.
  def upsert_fund_classification(row)
    return true unless row.is_a?(ActionController::Parameters)

    symbol = row[:symbol]
    return true if symbol.blank? || !FundClassification.kinds.key?(row[:kind])

    # Stored exactly as the ledger spells it. Upcasing or stripping here would make every later
    # FundClassification.resolve lookup miss and read the security back as unclassified.
    record = current_user.fund_classifications.find_or_initialize_by(symbol: symbol)
    record.kind = row[:kind]
    category = row[:fund_category]
    record.fund_category = (category if record.fund? && FundClassification.fund_categories.key?(category))
    record.save
  end

  def transfer_candidates(transaction)
    scope = AccountTransaction.for_user(current_user).where(base_currency: transaction.base_currency)
    at = transaction.transacted_at

    # A user-asserted link gets 14 days (not the auto-matcher's 72 hours) and no 2% tolerance.
    if transaction.withdrawal?
      # Claimed deposits must be excluded before update! reaches the unique index.
      claimed = AccountTransaction.for_user(current_user).where.not(linked_transaction_id: nil)
                                  .select(:linked_transaction_id)
      scope.deposit
           .where(transacted_at: at..(at + TRANSFER_LINK_WINDOW))
           .where(base_amount: ..transaction.base_amount)
           .where.not(id: claimed)
           .limit(2).to_a
    elsif transaction.deposit?
      scope.withdrawal
           .where(linked_transaction_id: nil)
           .where(transacted_at: (at - TRANSFER_LINK_WINDOW)..at)
           .where(base_amount: transaction.base_amount..)
           .limit(2).to_a
    else
      []
    end
  end

  # Read-only: a cold scope is warmed by a job and the page shows the bot's loading state until it
  # lands. Two scopes, because the page reads at two altitudes — the tiles and the chart are the
  # whole portfolio, the holdings card and the record follow the exchange switch.
  def load_ledgers
    @ledger = Tracker::Ledger.cached(current_user)
    Tracker::LedgerJob.perform_later(current_user.id, nil) if @ledger.nil?
    @scoped_ledger = @scope_exchange ? Tracker::Ledger.cached(current_user, exchange: @scope_exchange) : @ledger
    return if @scoped_ledger || @scope_exchange.nil?

    Tracker::LedgerJob.perform_later(current_user.id, @scope_exchange.id)
  end

  # The chart's series. Coverage decides the backfill, not "are there any snapshots": the nightly
  # sync only ever appends today, so a history that starts after the first transaction has a gap
  # nothing else will fill. A user whose first transaction is today has no gap yet.
  def load_history
    @history = PortfolioSnapshot.series(current_user, exchange: @scope_exchange)
    if @history.nil?
      # A venue's series is swept on demand: the chart shows its empty landscape until the job lands.
      PortfolioSnapshot::BackfillJob.perform_later(current_user.id, @scope_exchange.id)
      @history = []
      return
    end

    first_transaction = AccountTransaction.for_user(current_user).minimum(:transacted_at)&.to_date
    return if @scope_exchange || first_transaction.nil? || first_transaction >= Date.current
    return if @history.first && @history.first.date <= first_transaction

    PortfolioSnapshot::BackfillJob.perform_later(current_user.id)
  end

  def load_portfolio
    base = AccountBalance.for_user(current_user).nonzero.includes(:asset)
    base = base.for_exchange(@scope_exchange) if @scope_exchange
    balances = base.to_a

    priced, unpriced = balances.partition { |b| b.usd_value.to_d.positive? }

    @portfolio_slices = priced.group_by(&:asset).map do |asset, rows|
      { asset: asset, usd_value: rows.sum { |r| r.usd_value.to_d },
        quantity: rows.sum { |r| r.free.to_d + r.locked.to_d } }
    end.sort_by { |s| -s[:usd_value] }

    @portfolio_total_usd = @portfolio_slices.sum { |s| s[:usd_value] }
    @portfolio_unpriced_assets = unpriced.map(&:asset).uniq
    @portfolio_last_synced_at = balances.map(&:synced_at).compact.max
    @portfolio_oldest_priced_at = priced.map(&:priced_at).compact.min
    @portfolio_has_stale_prices = @portfolio_oldest_priced_at.present? &&
                                  @portfolio_last_synced_at.present? &&
                                  (@portfolio_last_synced_at - @portfolio_oldest_priced_at) > 5.minutes
    # Balances are priced and stored in USD; the denominator is the last step before display.
    @denomination = current_user.denomination
    @portfolio_has_keys = current_user.api_keys.where(key_type: :trading, status: :correct).exists?
    @portfolio_never_synced = @portfolio_has_keys && balances.empty? &&
                              !AccountBalance.for_user(current_user).exists?
  end

  # Reads the pending report's own key, not the export preferences: toggling a radio in the modal
  # rewrites preferences, and a report already generating must not become unfindable because of it.
  def check_pending_report
    pending = (current_user.tracker_settings || {})['pending_report']
    return unless pending.is_a?(Hash)

    country = pending['country']
    year = pending['year']
    report_scope = pending['report_scope'].presence || 'crypto'
    return unless country && year

    file_path = Tax::GenerateReportJob.report_path(current_user.id, country, year, report_scope)
    return unless File.exist?(file_path)

    # File exists — report finished while user was away. Set flag for auto-download.
    @pending_report = { country: country, year: year, report_scope: report_scope }
  end

  def validate_coingecko_api_key(api_key)
    return false if api_key.blank?

    coingecko = Coingecko.new(api_key: api_key)
    result = coingecko.get_top_coins_by_market_cap(limit: 5)
    result.success?
  end

  def filtered_transactions
    scope = AccountTransaction.for_user(current_user)
    scope = scope.for_exchange(Exchange.find(params[:exchange_id])) if params[:exchange_id].present?
    scope.in_date_range(
      params[:from].present? ? Date.parse(params[:from]).beginning_of_day : nil,
      params[:to].present? ? Date.parse(params[:to]).end_of_day : nil
    )
  end
end
