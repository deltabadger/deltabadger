class Transaction < ApplicationRecord
  belongs_to :bot
  enum currency: %i[USD EUR PLN]
  enum status: %i[success failure skipped]

  after_create :set_daily_transaction_aggregate

  validates :bot, presence: true

  def get_errors
    JSON.parse(error_messages)
  end

  def price
    return 0.0 unless rate.present?

    amount * rate
  end

  private

  # rubocop:disable Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity
  def set_daily_transaction_aggregate
    return unless success?

    transactions_repository = TransactionsRepository.new
    daily_transaction_aggregates_repository = DailyTransactionAggregateRepository.new
    daily_transaction_aggregate = daily_transaction_aggregates_repository.today_for_bot(bot).first
    return daily_transaction_aggregates_repository.create(attributes.except('id')) unless daily_transaction_aggregate

    bot_transactions = transactions_repository.today_for_bot(bot)
    bot_transactions_with_rate = bot_transactions.reject { |t| t.rate.nil? }
    bot_transactions_with_amount = bot_transactions.reject { |t| t.amount.nil? }
    return if bot_transactions_with_rate.count.zero? || bot_transactions_with_amount.count.zero?

    daily_transaction_aggregate_new_data = {
      rate: bot_transactions_with_rate.sum(&:rate) / bot_transactions_with_rate.count.to_f,
      amount: bot_transactions_with_amount.sum(&:amount)
    }
    daily_transaction_aggregates_repository.update(daily_transaction_aggregate.id, daily_transaction_aggregate_new_data)
  end
  # rubocop:enable Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity
end
