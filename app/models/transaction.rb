class Transaction < ApplicationRecord
  belongs_to :bot
  belongs_to :exchange

  before_create :set_exchange, if: -> { bot.legacy? }
  before_create :round_numeric_fields
  after_create_commit :set_daily_transaction_aggregate
  after_create_commit :update_bot_metrics, unless: -> { bot.legacy? }
  after_create_commit :stop_bot_and_notify_by_quote_amount_limit, if: -> { bot.barbell? }

  scope :for_bot, ->(bot) { where(bot_id: bot.id).order(created_at: :desc) }
  scope :today_for_bot, ->(bot) { for_bot(bot).where('created_at >= ?', Date.today.beginning_of_day) }
  scope :for_bot_by_status, ->(bot, status: :success) { where(bot_id: bot.id).where(status: status).order(created_at: :desc) }

  validates :bot, presence: true

  enum status: %i[success failure skipped]

  include Broadcastable

  BTC = %w[XXBT XBT BTC].freeze

  # TODO: Migrate Transaction & DailyTransactionAggregate to directly refference assets instead of symbols
  def base_asset
    @base_asset ||= exchange.assets.find_by(symbol: base) ||
                    exchange.tickers.find_by(base: base)&.base_asset ||
                    exchange.tickers.find_by(quote: base)&.quote_asset ||
                    Asset.find_by(symbol: base)
  end

  def quote_asset
    @quote_asset ||= exchange.assets.find_by(symbol: quote) ||
                     exchange.tickers.find_by(quote: quote)&.quote_asset ||
                     exchange.tickers.find_by(base: quote)&.base_asset ||
                     Asset.find_by(symbol: quote)
  end

  # def count_by_status_and_exchange(status, exchange)
  #   joins(:bot).where(bots: { exchange_id: exchange.id }).where(status: status).count
  # end

  # def total_btc_sold
  #   total_btc_by_type('sell')
  # end

  # def total_btc_bought_day_ago
  #   joins(:bot)
  #     .where("bots.settings->>'type' = 'buy' AND bots.settings->>'base' IN (?) AND transactions.created_at < ? ",
  #            BTC, 1.days.ago)
  #     .sum(:amount).ceil(8)
  # end

  private

  # def total_btc_by_type(type)
  #   joins(:bot)
  #     .where("bots.settings->>'type' = ? AND bots.settings->>'base' IN (?)", type, BTC)
  #     .sum(:amount).ceil(8)
  # end

  def set_exchange
    self.exchange ||= bot.exchange
  end

  def round_numeric_fields
    self.rate = rate&.round(18)
    self.amount = amount&.round(18)
    self.bot_price = bot_price&.round(18)
  end

  # rubocop:disable Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity
  def set_daily_transaction_aggregate
    return unless success?

    daily_transaction_aggregate = DailyTransactionAggregate.today_for_bot(bot).first
    return DailyTransactionAggregate.create(attributes.except('id', 'exchange_id')) unless daily_transaction_aggregate

    bot_transactions = Transaction.today_for_bot(bot)
    bot_transactions_with_rate = bot_transactions.reject { |t| t.rate.nil? }
    bot_transactions_with_amount = bot_transactions.reject { |t| t.amount.nil? }
    return if bot_transactions_with_rate.count.zero? || bot_transactions_with_amount.count.zero?

    daily_transaction_aggregate_new_data = {
      rate: bot_transactions_with_rate.sum(&:rate) / bot_transactions_with_rate.count.to_f,
      amount: bot_transactions_with_amount.sum(&:amount)
    }
    daily_transaction_aggregate.update(daily_transaction_aggregate_new_data)
  end
  # rubocop:enable Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity

  def update_bot_metrics
    Bot::UpdateMetricsJob.perform_later(bot)
  end

  def stop_bot_and_notify_by_quote_amount_limit
    return unless success? && bot.quote_amount_limited? && bot.quote_amount_limit_reached?

    bot.stop
    bot.notify_stopped_by_quote_amount_limit

    # after stopping outside of the controller, we need to broadcast the streams the same way as
    # app/views/bots/stop.turbo_stream.erb
    broadcast_replace_to(
      ["user_#{bot.user_id}", :bot_updates],
      target: 'settings',
      partial: 'bots/barbell/settings',
      locals: { bot: bot }
    )
    broadcast_replace_to(
      ["user_#{bot.user_id}", :bot_updates],
      target: 'exchange_select',
      partial: 'bots/exchange_select',
      locals: { bot: bot }
    )
  end
end
