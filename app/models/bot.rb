class Bot < ApplicationRecord
  belongs_to :exchange, optional: true
  belongs_to :user
  has_many :transactions, dependent: :destroy
  has_many :daily_transaction_aggregates

  enum status: %i[created scheduled stopped deleted executing retrying]

  scope :working, -> { where(status: %i[scheduled executing retrying]) }
  scope :legacy, -> { where(type: %w[Bots::Basic Bots::Withdrawal Bots::Webhook]) }
  scope :not_legacy, -> { where.not(type: %w[Bots::Basic Bots::Withdrawal Bots::Webhook]) }

  include Typeable
  include Labelable
  include Webhookable
  include Rankable
  include Notifyable
  include DomIdable

  delegate :market_sell, :market_buy, :limit_sell, :limit_buy, to: :exchange

  before_save :update_settings_changed_at, if: :will_save_change_to_settings?
  after_update_commit :broadcast_status_bar_update, if: :saved_change_to_status?
  after_update_commit :broadcast_status_button_update, if: :saved_change_to_status?

  INTERVALS = %w[hour day week month].freeze

  def legacy?
    ['Bots::Basic', 'Bots::Withdrawal', 'Bots::Webhook'].include?(type)
  end

  def working?
    scheduled? || executing? || retrying?
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
    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: dom_id(self, :status_bar),
      partial: 'bots/status/status_bar',
      locals: { bot: self }
    )
  end

  def broadcast_status_button_update
    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: dom_id(self, :status_button),
      partial: legacy? ? 'bots/status/status_button_legacy' : 'bots/status/status_button',
      locals: { bot: self }
    )
  end

  private

  def update_settings_changed_at
    self.settings_changed_at = Time.current
  end
end
