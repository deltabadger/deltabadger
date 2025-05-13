class Transaction < ApplicationRecord
  belongs_to :bot
  belongs_to :exchange

  before_create :set_exchange, if: -> { bot.legacy? }
  before_create :round_numeric_fields
  after_create_commit :set_daily_transaction_aggregate
  after_create_commit :update_bot_metrics, unless: -> { bot.legacy? }
  after_create_commit :broadcast_to_bot, unless: -> { bot.legacy? }
  after_create_commit :broadcast_below_minimums_warning_to_bot, unless: -> { bot.legacy? }

  scope :for_bot, ->(bot) { where(bot_id: bot.id).order(created_at: :desc) }
  scope :today_for_bot, ->(bot) { for_bot(bot).where('created_at >= ?', Date.today.beginning_of_day) }
  scope :for_bot_by_status, ->(bot, status: :success) { where(bot_id: bot.id).where(status: status).order(created_at: :desc) }

  validates :bot, presence: true

  enum status: %i[success failure skipped]

  BTC = %w[XXBT XBT BTC].freeze

  def quote_amount
    return nil unless amount.present? && rate.present?

    amount * rate
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

  def broadcast_to_bot
    # TODO: When transactions point to real asset ids, we can use the asset ids directly instead of symbols
    ticker = exchange.tickers.find_by(base_asset: base_asset, quote_asset: quote_asset)
    decimals = {
      base_asset.symbol => ticker.base_decimals,
      quote_asset.symbol => ticker.quote_decimals
    }

    if bot.transactions.limit(2).count == 1
      broadcast_remove_to(
        ["user_#{bot.user_id}", :bot_updates],
        target: 'orders_list_placeholder'
      )
    end

    broadcast_prepend_to(
      ["user_#{bot.user_id}", :bot_updates],
      target: 'orders_list',
      partial: 'bots/orders/order',
      locals: { order: self, decimals: decimals, exchange_name: exchange.name }
    )
  end

  def broadcast_below_minimums_warning_to_bot
    first_transactions = bot.transactions.limit(3)
    return unless first_transactions.count == 2
    return unless [first_transactions.first.skipped?, first_transactions.last.skipped?].any?

    first_transaction = first_transactions.first
    second_transaction = first_transactions.last

    locals = if first_transaction.skipped? && second_transaction.skipped?
               ticker0 = first_transaction.exchange.tickers.find_by(
                 base_asset_id: first_transaction.base_asset.id,
                 quote_asset_id: first_transaction.quote_asset.id
               )
               ticker1 = second_transaction.exchange.tickers.find_by(
                 base_asset_id: second_transaction.base_asset.id,
                 quote_asset_id: second_transaction.quote_asset.id
               )
               {
                 base0_symbol: first_transaction.base_asset.symbol,
                 base1_symbol: second_transaction.base_asset.symbol,
                 base0_minimum_base_size: ticker0.minimum_base_size,
                 base0_minimum_quote_size: ticker0.minimum_quote_size,
                 quote_symbol: first_transaction.quote_asset.symbol,
                 base1_minimum_base_size: ticker1.minimum_base_size,
                 base1_minimum_quote_size: ticker1.minimum_quote_size,
                 exchange_name: first_transaction.exchange.name,
                 missed_count: 2
               }
             else
               bought_transaction = first_transaction.skipped? ? second_transaction : first_transaction
               missed_transaction = first_transaction.skipped? ? first_transaction : second_transaction
               {
                 bought_quote_amount: bought_transaction.quote_amount,
                 quote_symbol: bought_transaction.quote_asset.symbol,
                 bought_symbol: bought_transaction.base_asset.symbol,
                 missed_symbol: missed_transaction.base_asset.symbol,
                 missed_minimum_base_size: missed_transaction.base_asset.min_base_size,
                 missed_minimum_quote_size: missed_transaction.base_asset.min_quote_size,
                 exchange_name: first_transaction.exchange.name,
                 missed_count: 1
               }
             end

    broadcast_replace_to(
      ["user_#{bot.user_id}", :bot_updates],
      target: 'modal',
      partial: 'bots/barbell/warning_below_minimums',
      locals: locals
    )
  end
end
