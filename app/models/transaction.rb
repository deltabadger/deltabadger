class Transaction < ApplicationRecord
  belongs_to :bot
  belongs_to :exchange

  before_create :set_exchange, if: -> { bot.legacy? }
  before_create :round_numeric_fields
  after_create_commit :set_daily_transaction_aggregate
  after_create_commit -> { bot.broadcast_new_order(self) unless bot.legacy? }
  after_create_commit -> { Bot::UpdateMetricsJob.perform_later(bot) unless bot.legacy? }
  after_create_commit -> { bot.handle_quote_amount_limit_update if submitted? && bot.class.include?(Bot::QuoteAmountLimitable) }

  scope :for_bot, ->(bot) { where(bot_id: bot.id).order(created_at: :desc) }
  scope :today_for_bot, ->(bot) { for_bot(bot).where('created_at >= ?', Date.today.beginning_of_day) }
  scope :for_bot_by_status, ->(bot, status: :submitted) { where(bot_id: bot.id).where(status: status).order(created_at: :desc) }

  validates :bot, presence: true
  validates :filled_percentage, numericality: { greater_than_or_equal_to: 0, less_than_or_equal_to: 1 }, allow_nil: true

  enum status: [
    :submitted, # Successfully sent and accepted by the exchange
    :failed,    # Attempted but failed (internal error or exchange rejection)
    :skipped    # Not even attempted (e.g., filtered, blocked, etc.)
  ]
  enum side: %i[buy sell]
  enum order_type: %i[market_order limit_order]
  enum external_status: %i[unknown open closed]

  BTC = %w[XXBT XBT BTC].freeze

  def filled?
    filled_percentage == 1
  end

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
    self.price = price&.round(18)
    self.amount = amount&.round(18)
    self.bot_quote_amount = bot_quote_amount&.round(18)
  end

  def set_daily_transaction_aggregate # rubocop:disable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity
    return unless submitted?

    daily_transaction_aggregate = DailyTransactionAggregate.today_for_bot(bot).first
    return DailyTransactionAggregate.create(attributes.except('id', 'exchange_id')) unless daily_transaction_aggregate

    bot_transactions = Transaction.today_for_bot(bot)
    bot_transactions_with_price = bot_transactions.reject { |t| t.price.nil? }
    bot_transactions_with_amount = bot_transactions.reject { |t| t.amount.nil? }
    return if bot_transactions_with_price.count.zero? || bot_transactions_with_amount.count.zero?

    daily_transaction_aggregate_new_data = {
      price: bot_transactions_with_price.sum(&:price) / bot_transactions_with_price.count.to_f,
      amount: bot_transactions_with_amount.sum(&:amount)
    }
    daily_transaction_aggregate.update(daily_transaction_aggregate_new_data)
  end
end
