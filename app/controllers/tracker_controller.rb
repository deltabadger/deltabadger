class TrackerController < ApplicationController
  before_action :authenticate_user!

  def index
    exchange_ids = (current_user.api_keys.pluck(:exchange_id) +
      current_user.account_transactions.distinct.pluck(:exchange_id)).uniq
    @exchanges = Exchange.where(id: exchange_ids).order(:name)
    @exchanges_with_valid_keys = current_user.api_keys.where(key_type: :trading, status: :correct).pluck(:exchange_id).to_set
    @has_syncable_keys = @exchanges_with_valid_keys.any?
    @addable_exchanges = Exchange.available.where.not(id: exchange_ids).order(:name)
    user_transactions = AccountTransaction.for_user(current_user)
    @date_from = params[:from].presence || user_transactions.minimum(:transacted_at)&.to_date&.iso8601
    @date_to = params[:to].presence || Date.current.iso8601
    @account_transactions = filtered_transactions.by_date.includes(:exchange, :bot_transaction)
    load_portfolio
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
      params.permit(:export_type, :country, :year).to_h.compact_blank
    )
    current_user.update(tracker_settings: settings)
    head :ok
  end

  def tax_report
    country = params[:country]
    year = params[:year].to_i
    jurisdiction = Tax::Jurisdictions.for(country)

    unless jurisdiction
      redirect_to tracker_path, alert: t('tracker.tax_report.invalid_country')
      return
    end

    current_user.update(tracker_settings: (current_user.tracker_settings || {}).merge(
      'export_type' => 'tax_report', 'country' => country, 'year' => year
    ))

    stablecoin_as_fiat = params[:stablecoin_as_fiat] == 'true'
    Tax::GenerateReportJob.perform_later(current_user.id, country, year, stablecoin_as_fiat)

    render turbo_stream: turbo_stream.append('flash', partial: 'tracker/report_progress')
  end

  def download_tax_report
    country = params[:country]
    year = params[:year].to_i
    file_path = Rails.root.join('tmp', 'tax_reports', "#{current_user.id}_#{country}_#{year}.csv")

    if File.exist?(file_path)
      csv_data = File.read(file_path)
      File.delete(file_path)
      filename = "deltabadger-tax-report-#{country.downcase}-#{year}.csv"
      send_data csv_data, filename: filename, type: 'text/csv; charset=utf-8'
    else
      redirect_to tracker_path, alert: t('tracker.tax_report.expired')
    end
  end

  private

  def load_portfolio
    base = AccountBalance.for_user(current_user).nonzero.includes(:asset)
    base = base.for_exchange(Exchange.find(params[:exchange_id])) if params[:exchange_id].present?
    balances = base.to_a

    priced, unpriced = balances.partition { |b| b.usd_value.to_d.positive? }

    @portfolio_slices = priced.group_by(&:asset).map do |asset, rows|
      { asset: asset, usd_value: rows.sum { |r| r.usd_value.to_d } }
    end.sort_by { |s| -s[:usd_value] }

    @portfolio_total_usd = @portfolio_slices.sum { |s| s[:usd_value] }
    @portfolio_unpriced_assets = unpriced.map(&:asset).uniq
    @portfolio_last_synced_at = balances.map(&:synced_at).compact.max
    @portfolio_oldest_priced_at = priced.map(&:priced_at).compact.min
    @portfolio_has_stale_prices = @portfolio_oldest_priced_at.present? &&
                                  @portfolio_last_synced_at.present? &&
                                  (@portfolio_last_synced_at - @portfolio_oldest_priced_at) > 5.minutes
    @portfolio_has_keys = current_user.api_keys.where(key_type: :trading, status: :correct).exists?
    @portfolio_never_synced = @portfolio_has_keys && balances.empty? &&
                              !AccountBalance.for_user(current_user).exists?
  end

  def check_pending_report
    settings = current_user.tracker_settings || {}
    return unless settings['export_type'] == 'tax_report'

    country = settings['country']
    year = settings['year']
    return unless country && year

    file_path = Rails.root.join('tmp', 'tax_reports', "#{current_user.id}_#{country}_#{year}.csv")
    return unless File.exist?(file_path)

    # File exists — report finished while user was away. Set flag for auto-download.
    @pending_report = { country: country, year: year }
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
