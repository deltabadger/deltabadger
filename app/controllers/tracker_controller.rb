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
    transactions = filtered_transactions
    csv_data = AccountTransaction.to_csv(transactions)
    filename = "deltabadger-tax-export-#{Date.current.iso8601}.csv"
    send_data csv_data, filename: filename, type: 'text/csv; charset=utf-8'
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
