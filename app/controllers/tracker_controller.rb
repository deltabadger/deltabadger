class TrackerController < ApplicationController
  before_action :authenticate_user!

  def index
    @account_transactions = filtered_transactions.by_date.includes(:exchange, :bot_transaction)
    @exchanges = Exchange.where(id: current_user.api_keys.select(:exchange_id)).order(:name)
  end

  def sync
    current_user.api_keys.where(key_type: :trading, status: :correct).each_with_index do |api_key, i|
      AccountTransaction::SyncJob.set(wait: i * 30.seconds).perform_later(api_key)
    end
    redirect_to tracker_index_path, notice: t('tracker.sync_started')
  end

  def export
    current_user.update(tracker_settings: (current_user.tracker_settings || {}).merge('export_type' => 'transactions'))

    transactions = filtered_transactions
    csv_data = AccountTransaction.to_csv(transactions)
    filename = "deltabadger-transactions-#{Date.current.iso8601}.csv"
    send_data csv_data, filename: filename, type: 'text/csv; charset=utf-8'
  end

  def export_modal
    @settings = current_user.tracker_settings || {}
    @jurisdictions = Tax::Jurisdictions.available
    render layout: false
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
      redirect_to tracker_index_path, alert: t('tracker.tax_report.invalid_country')
      return
    end

    current_user.update(tracker_settings: (current_user.tracker_settings || {}).merge(
      'export_type' => 'tax_report', 'country' => country, 'year' => year
    ))

    Tax::GenerateReportJob.perform_later(current_user.id, country, year)

    render turbo_stream: turbo_stream.append('flash', partial: 'tracker/report_progress')
  end

  def download_tax_report
    country = params[:country]
    year = params[:year].to_i
    file_path = Rails.root.join('tmp', 'tax_reports', "#{current_user.id}_#{country}_#{year}.csv")

    if File.exist?(file_path)
      csv_data = File.read(file_path)
      filename = "deltabadger-tax-report-#{country.downcase}-#{year}.csv"
      send_data csv_data, filename: filename, type: 'text/csv; charset=utf-8'
    else
      redirect_to tracker_index_path, alert: t('tracker.tax_report.expired')
    end
  end

  private

  def filtered_transactions
    scope = AccountTransaction.for_user(current_user)
    scope = scope.for_exchange(Exchange.find(params[:exchange_id])) if params[:exchange_id].present?
    scope.in_date_range(
      params[:from].present? ? Date.parse(params[:from]).beginning_of_day : nil,
      params[:to].present? ? Date.parse(params[:to]).end_of_day : nil
    )
  end
end
