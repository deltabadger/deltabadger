class Bot < ApplicationRecord
  include Automation::Statusable
  include Automation::Configurable
  include Automation::Executable
  include Automation::ExchangeConnectable
  include Automation::Dryable # decorators for: api_key
  include Automation::Labelable
  include Automation::DomIdable

  include Typeable
  include Rankable
  include Notifyable
  include ExchangeUser

  belongs_to :user
  has_many :transactions, dependent: :destroy

  before_save :store_previous_exchange_id
  after_update_commit :broadcast_status_bar_update, if: -> { saved_change_to_status? && !@skip_status_bar_broadcast }
  after_update_commit :broadcast_status_button_update, if: :saved_change_to_status?
  after_update_commit :broadcast_columns_lock_update, if: :saved_change_to_status?
  after_update_commit -> { Bot::UpdateMetricsJob.perform_later(self) if custom_exchange_id_changed? }

  def last_transaction
    transactions.where(transaction_type: 'REGULAR').order(created_at: :desc).limit(1).last
  end

  def last_successful_transaction
    transactions.where(status: %i[submitted skipped]).order(created_at: :desc).limit(1).last
  end

  def successful_transaction_count
    transactions.where(status: %i[submitted skipped]).order(created_at: :desc).count
  end

  def total_amount
    transactions.submitted.sum(:amount)
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
      partial: 'bots/status/status_button',
      locals: { bot: self }
    )
  end

  def broadcast_columns_lock_update
    action = working? ? :add_class : :remove_class
    Turbo::StreamsChannel.broadcast_action_to(
      ["user_#{user_id}", :bot_updates],
      action:,
      target: dom_id(self, :columns),
      attributes: { 'class-name': 'bot-locked' }
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
      locals: { order:, decimals:, exchange_name: order.exchange.name, current_user: user, fetch: false }
    )

    broadcast_order_filters_update
  end

  def broadcast_order_filters_update
    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: dom_id(self, :order_filters),
      partial: 'bots/orders/order_filters',
      locals: { bot: self }
    )
  end

  def broadcast_updated_order(order)
    # TODO: When transactions point to real asset ids, we can use the asset ids directly instead of symbols
    ticker = order.exchange.tickers.find_by(base_asset: order.base_asset, quote_asset: order.quote_asset)
    decimals = {
      order.base_asset.symbol => ticker.base_decimals,
      order.quote_asset.symbol => ticker.quote_decimals
    }

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: dom_id(order),
      partial: 'bots/orders/order',
      locals: { order:, decimals:, exchange_name: order.exchange.name, current_user: user, fetch: false }
    )

    broadcast_order_filters_update
  end

  # Override broadcast methods to use user's locale for translated partials
  def broadcast_replace_to(...)
    with_user_locale { super }
  end

  def broadcast_prepend_to(...)
    with_user_locale { super }
  end

  def broadcast_append_to(...)
    with_user_locale { super }
  end

  private

  def with_user_locale(&block)
    I18n.with_locale(user.locale || I18n.default_locale, &block)
  end

  def store_previous_exchange_id
    @previous_exchange_id = exchange_id_was
  end

  def custom_exchange_id_changed?
    exchange_id != @previous_exchange_id
  end
end
