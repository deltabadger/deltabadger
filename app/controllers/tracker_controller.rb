class TrackerController < ApplicationController
  include AdminOnly

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
    # What the + can still add. Retired venues are out — there is nothing left to connect to —
    # and so is anything already on the switch. Brokers lead: they are the half of the list
    # someone arriving from a stock holding is looking for, and there are only ever a few.
    @connectable_exchanges = Exchange.tradeable.where.not(id: @exchanges.select(:id))
                                     .order(:name).partition(&:stock_venue?).flatten
    @exchanges_with_valid_keys = reading_keys.to_set(&:exchange_id)
    @has_syncable_keys = @exchanges_with_valid_keys.any?
    user_transactions = AccountTransaction.for_user(current_user)
    @date_from = params[:from].presence || user_transactions.minimum(:transacted_at)&.to_date&.iso8601
    @date_to = params[:to].presence || Date.current.iso8601
    rows = filtered_transactions.by_date.includes(:exchange, :bot_transaction, :linked_transaction, :inverse_link)
    @account_transactions = params[:all].present? ? rows : rows.limit(ROW_LIMIT)
    # A sync failure survives the page it was broadcast onto, so the banner has to be rebuilt on
    # load — otherwise a persisted failure is invisible until the next sync. Only `:failed`: a
    # never-synced key is `sync_issue`'s other reason and is not a failure to shout about here.
    #
    # And only from the key this venue is READ WITH. `last_sync_error` is a note left on a key and
    # erased only when that key syncs again, so a key the tracker has stopped using keeps its note
    # forever — a rejected trading key would warn that Binance history is missing on a page showing
    # that history, read through the key beside it. A venue with no working key has no such
    # replacement, so its failure still speaks.
    read_with = reading_keys.index_by(&:exchange_id)
    @sync_failures = current_user.api_keys.includes(:exchange).filter_map do |api_key|
      next if read_with[api_key.exchange_id] && read_with[api_key.exchange_id] != api_key

      issue = api_key.sync_issue
      issue[:exchange] if issue && issue[:reason] == :failed
    end
    load_ledgers
    load_portfolio
    load_history
    check_pending_report
  end

  def sync
    result = BotApi::Tracker::Sync.call(user: current_user)
    return head :no_content unless result.success?

    render turbo_stream: turbo_stream.append(
      'flash', partial: 'tracker/sync_progress',
               locals: { exchange_name: result.data[:exchanges].join(', ') }
    )
  end

  # One implementation: BotApi::Tracker::SetTransferLink also backs the MCP tool and the REST
  # endpoint, so the candidate rule, the sticky unlink flag and the snapshot rebuild are decided
  # in one place.
  def toggle_transfer
    transaction = AccountTransaction.for_user(current_user).find(params[:id])
    result = BotApi::Tracker::SetTransferLink.call(user: current_user, transaction_id: transaction.id,
                                                   linked: !transaction.linked?)

    if result.success?
      flash.now[:notice] = t(result.data[:linked] ? 'tracker.transfer_linked' : 'tracker.transfer_unlinked')
      rows = AccountTransaction.where(id: result.data.values_at(:withdrawal_id, :deposit_id)).to_a
    else
      flash.now[:alert] = t('tracker.transfer_no_candidate')
      rows = [transaction]
    end

    streams = rows.compact.map do |row|
      turbo_stream.replace(
        helpers.dom_id(row),
        partial: 'tracker/transaction_row',
        locals: { at: row.reload }
      )
    end
    render turbo_stream: streams + [turbo_stream_prepend_flash]
  end

  # A price the user states for one row. Everything that reads a priced row goes through
  # `PriceService#enrich`, which prefers a stated price — so the tiles, the chart, the positions and
  # the tax report all inherit this without knowing it exists. The box is in the reader's own
  # currency, like the Value beside it; the ledger counts in USD. Convert on the way in exactly as
  # the view converts on the way out, or a price typed in euro would be banked as dollars.
  def update_price
    transaction = AccountTransaction.for_user(current_user).find(params[:id])
    # A row with a price of its own has nothing to state, and a request that tries anyway is
    # refused by the service rather than stored and ignored.
    #
    # Validated before conversion: to_usd(nil) is nil, which set_manual reads as "clear", so an
    # unparseable figure would silently delete the stated price instead of being refused.
    raw = params[:price].to_s
    usd = if raw.blank?
            ''
          elsif (number = BotApi::Number.parse(raw))
            current_user.denomination.to_usd(number).to_d.to_s('F')
          end
    return head :unprocessable_entity if usd.nil?

    result = BotApi::Tracker::SetTransactionPrice.call(user: current_user, transaction_id: transaction.id,
                                                       price_usd: usd)
    return head :unprocessable_entity unless result.success?

    transaction.reload
    render turbo_stream: turbo_stream.replace(helpers.dom_id(transaction),
                                              partial: 'tracker/transaction_row',
                                              locals: { at: transaction })
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

  # Cash and stablecoins are where money waits, not a position anybody picked, so the page is about
  # the active positions unless asked otherwise. Absent means off: the default is the invested
  # portfolio, and `Hide balances` is the separate switch for taking the money off the screen.
  def show_cash? = current_user.show_cash?
  helper_method :show_cash?

  # The page only ever reads, so every key that can read serves it — at most one per venue. Memoized
  # because `index` asks three times.
  def reading_keys
    @reading_keys ||= ApiKey.reading(current_user.api_keys.includes(:exchange))
  end

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
      # A venue's series is swept on demand: the chart spins until the job lands.
      PortfolioSnapshot::BackfillJob.perform_later(current_user.id, @scope_exchange.id)
      @history = []
      @history_loading = true
      return
    end

    first_transaction = AccountTransaction.for_user(current_user).minimum(:transacted_at)&.to_date
    # The curve follows the switch too, and a day swept before it cannot answer for the positions —
    # the cash standing on it is not recoverable from a row that never stated it. Those days are not
    # drawn (a nil read as zero would be a lie in the shape of a figure), and a sweep is asked for
    # while any of them is a day a sweep can still reach: the first transaction to yesterday, which
    # is exactly the window the job writes. Nothing else is asked for on every page load — a row
    # older than the account's own history is dropped for good, and TODAY's row is the sync's to
    # rewrite (`record!` states both readings), which it does on the next one.
    unless show_cash?
      unswept, @history = @history.partition { |row| row.held_cost_usd.nil? }
      if unswept.any? { |row| first_transaction && row.date.between?(first_transaction, Date.yesterday) }
        PortfolioSnapshot::BackfillJob.perform_later(current_user.id, @scope_exchange&.id)
        @history_loading = @history.empty?
        return
      end
    end

    # A day already swept can still be wrong: it was valued at the prices that existed then, and the
    # sweep only ever runs forwards. Prices arriving since — a range the provider could not answer
    # before — change days nothing else revisits, so a history with unvalued days asks for one more
    # pass. Bounded by the generation: once swept, it does not ask again until a price actually
    # arrives, so a day no price can fix does not re-enqueue on every page load.
    if PortfolioSnapshot.stale_prices?(current_user)
      PortfolioSnapshot::BackfillJob.perform_later(current_user.id)
      return
    end

    return if @scope_exchange || first_transaction.nil? || first_transaction >= Date.current
    return if @history.first && @history.first.date <= first_transaction

    @history_loading = @history.empty?
    PortfolioSnapshot::BackfillJob.perform_later(current_user.id)
  end

  def load_portfolio
    base = AccountBalance.for_user(current_user).nonzero.includes(:asset)
    base = base.for_exchange(@scope_exchange) if @scope_exchange
    balances = base.to_a

    priced, unpriced = balances.partition { |b| b.usd_value.to_d.positive? }

    @portfolio_unpriced_assets = unpriced.map(&:asset).uniq
    # A venue whose last sync found nothing still synced: its watermark stands where its rows do not.
    marks = PortfolioSnapshot.watermarks(current_user, @scope_exchange).values
    @portfolio_last_synced_at = (balances.map(&:synced_at) + marks).compact.max
    @portfolio_oldest_priced_at = priced.map(&:priced_at).compact.min
    @portfolio_has_stale_prices = @portfolio_oldest_priced_at.present? &&
                                  @portfolio_last_synced_at.present? &&
                                  (@portfolio_last_synced_at - @portfolio_oldest_priced_at) > 5.minutes
    # Balances are priced and stored in USD; the denominator is the last step before display.
    @denomination = current_user.denomination
    # ONE source for every figure the page states. The templates used to each work out their own
    # from two different truths — the ledger and the balances — which is how they came to
    # contradict each other with nothing able to notice.
    @figures = Tracker::Figures.for(current_user, ledger: @scoped_ledger, balances: balances,
                                                  pending: quantities_since)
    # The switch beside the venues, applied once — and it scopes the whole page, not only the list:
    # what is drawn FROM the holdings (the card, the ring, the type shares, the positions table)
    # and what is stated ABOUT them (money in, value, the P/L between the two). See
    # `Figures::Result#without_cash` for what that restatement is.
    unless show_cash?
      invested = @figures.without_cash
      # An account holding nothing BUT cash has balances; it just has no invested position to
      # draw. Without this the holdings card would say none were found, which is a lie the reader
      # cannot act on — the money is stated in the tiles right beside it.
      @portfolio_all_cash = @figures.holdings.any? && invested.holdings.empty?
      @figures = invested
    end
    @portfolio_has_keys = reading_keys.any?
    @portfolio_never_synced = @portfolio_has_keys && balances.empty? &&
                              PortfolioSnapshot.watermarks(current_user).values.compact.empty?
  end

  # What the ledger has seen since the balances were taken. A balance is a snapshot and the bots go
  # on trading: without this, every asset bought since the last sync looks like a holding whose
  # quantity the ledger cannot vouch for, and the page withholds a P/L it actually knows.
  def quantities_since
    Tracker::Figures.moved_since(PortfolioSnapshot.pending_scope(current_user, @scope_exchange),
                                 PortfolioSnapshot.watermarks(current_user, @scope_exchange))
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
