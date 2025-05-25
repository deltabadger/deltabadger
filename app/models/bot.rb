class Bot < ApplicationRecord
  belongs_to :exchange, optional: true
  belongs_to :user
  has_many :transactions, dependent: :destroy
  has_many :daily_transaction_aggregates

  enum status: %i[created scheduled stopped deleted executing retrying waiting]

  scope :working, -> { where(status: %i[scheduled executing retrying waiting]) }
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

  def interval_duration
    return nil unless interval.present?

    1.public_send(interval)
  end

  def legacy?
    ['Bots::Basic', 'Bots::Withdrawal', 'Bots::Webhook'].include?(type)
  end

  def working?
    scheduled? || executing? || retrying? || waiting?
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

  def broadcast_new_order(order)
    # TODO: When transactions point to real asset ids, we can use the asset ids directly instead of symbols
    ticker = order.exchange.tickers.find_by(base_asset: order.base_asset, quote_asset: order.quote_asset)
    decimals = {
      order.base_asset.symbol => ticker.base_decimals,
      order.quote_asset.symbol => ticker.quote_decimals
    }

    if transactions.limit(2).count == 1
      broadcast_remove_to(
        ["user_#{user_id}", :bot_updates],
        target: 'orders_list_placeholder'
      )
    end

    broadcast_prepend_to(
      ["user_#{user_id}", :bot_updates],
      target: 'orders_list',
      partial: 'bots/orders/order',
      locals: { order: order, decimals: decimals, exchange_name: order.exchange.name, current_user: user }
    )
  end

  private

  def update_settings_changed_at
    # FIXME: Required because we are using store_accessor and will_save_change_to_settings?
    # always returns true, at least in Rails 6.0
    return if settings_was == settings

    self.settings_changed_at = Time.current
  end
end
