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

  def set_daily_transaction_aggregate
    return unless success?
    
    transactions_repository = TransactionsRepository.new
    daily_transaction_aggregates_repository = DailyTransactionAggregateRepository.new
    daily_transaction_aggregate = daily_transaction_aggregates_repository.today_for_bot(bot).first
    
    bot_transactions = transactions_repository.today_for_bot(bot)
  
    if bot_transactions.count.zero?
      daily_transaction_aggregate_new_data = { rate: rate, amount: amount }
    else
      daily_transaction_aggregate_new_data = { rate: bot_transactions.sum(&:rate)/bot_transactions.count, amount: bot_transactions.sum(&:amount) }
    end
  
    if daily_transaction_aggregate
      daily_transaction_aggregates_repository.update(daily_transaction_aggregate.id, daily_transaction_aggregate_new_data)
    else
      daily_transaction_aggregates_repository.create(attributes.except('id').merge(daily_transaction_aggregate_new_data))
    end
  end
  
end
