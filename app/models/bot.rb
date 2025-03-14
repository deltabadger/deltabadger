class Bot < ApplicationRecord
  belongs_to :exchange, optional: true
  belongs_to :user
  has_many :transactions, dependent: :destroy
  has_many :daily_transaction_aggregates

  enum status: %i[created working stopped deleted pending retrying]
  enum metrics_status: %i[unknown pending ready], _prefix: :metrics

  include Typeable
  include Labelable
  include Webhookable
  include Rankable
  include Notifyable

  delegate :market_sell, :market_buy, :limit_sell, :limit_buy, to: :exchange

  after_save :update_settings_changed_at, if: :saved_change_to_settings?
  after_update_commit :broadcast_status_bar_update, if: :saved_change_to_status?
  after_update_commit :broadcast_status_button_update, if: :saved_change_to_status?

  INTERVALS = %w[hour day week month].freeze

  def legacy?
    ['Bots::Basic', 'Bots::Withdrawal', 'Bots::Webhook'].include?(type)
  end

  def last_transaction
    transactions.where(transaction_type: 'REGULAR').order(created_at: :desc).limit(1).last
  end

  def last_successful_transaction
    transactions.where(status: %i[success skipped]).order(created_at: :desc).limit(1).last
  end

  def successful_transaction_count
    transactions.where(status: %i[success skipped]).order(created_at: :desc).count
  end

  def any_last_transaction
    transactions.order(created_at: :desc).limit(1).last
  end

  def last_withdrawal
    transactions.where(transaction_type: 'WITHDRAWAL').order(created_at: :desc).limit(1).last
  end

  def total_amount
    daily_transaction_aggregates.sum(:amount)
  end

  def destroy
    update(status: 'deleted')
  end

  def broadcast_status_bar_update
    broadcast_update_to(
      ["bot_#{id}", :status_bar],
      target: "bot_#{id}_status_bar",
      partial: 'barbell_bots/status/status_bar',
      locals: { bot: self }
    )
  end

  def broadcast_status_button_update
    broadcast_update_to(
      ["bot_#{id}", :status_button],
      target: "bot_#{id}_status_button",
      partial: legacy? ? 'barbell_bots/status/status_button_legacy' : 'barbell_bots/status/status_button',
      locals: { bot: self }
    )
  end

  private

  def update_settings_changed_at
    update!(settings_changed_at: Time.current)
  end
end
