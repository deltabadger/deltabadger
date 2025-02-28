class Transaction < ApplicationRecord
  belongs_to :bot

  after_create_commit :set_daily_transaction_aggregate
  after_create_commit :update_bot_metrics
  after_create_commit :broadcast_to_bot

  scope :for_bot, ->(bot) { where(bot_id: bot.id).order(created_at: :desc) }
  scope :today_for_bot, ->(bot) { for_bot(bot).where('created_at >= ?', Date.today.beginning_of_day) }
  scope :for_bot_by_status, ->(bot, status: :success) { where(bot_id: bot.id).where(status: status).order(created_at: :desc) }

  validates :bot, presence: true

  enum status: %i[success failure skipped]

  BTC = %w[XXBT XBT BTC].freeze

  def price
    return 0.0 unless rate.present?

    amount * rate
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

  # rubocop:disable Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity
  def set_daily_transaction_aggregate
    return unless success?

    daily_transaction_aggregate = DailyTransactionAggregate.today_for_bot(bot).first
    return DailyTransactionAggregate.create(attributes.except('id')) unless daily_transaction_aggregate

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
    Bot::UpdateMetricsJob.perform_later(bot_id)
  end

  def broadcast_to_bot
    if Transaction.for_bot(bot).count == 1
      broadcast_refresh_to(
        ["bot_#{bot_id}", :orders]
      )
    else
      broadcast_prepend_to(
        ["bot_#{bot_id}", :orders],
        target: "bot_#{bot_id}_orders",
        partial: 'barbell_bots/orders/order',
        locals: { order: self }
      )
    end
  end
end
