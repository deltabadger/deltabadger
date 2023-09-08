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

    # initialize repositories
    transactions_repository = TransactionsRepository.new
    daily_transaction_aggregates_repository = DailyTransactionAggregateRepository.new

    # get today's and last aggregates
    daily_transaction_aggregate = daily_transaction_aggregates_repository.today_for_bot(bot).first
    last_aggregate = daily_transaction_aggregate || daily_transaction_aggregates_repository.model.where(bot: bot).last

    # get today's transactions
    bot_transactions = transactions_repository.today_for_bot(bot)
    return if bot_transactions.empty?

    # calculate new data
    total_amount = (last_aggregate&.total_amount || 0) + bot_transactions.sum(&:amount)
    total_invested = (last_aggregate&.total_invested || 0) + bot_transactions.sum(&:price)

    new_data = {
      rate: bot_transactions.sum(&:rate) / bot_transactions.count,
      amount: bot_transactions.sum(&:amount),
      total_amount: total_amount,
      total_invested: total_invested,
      total_value: total_amount * (bot_transactions.sum(&:rate) / bot_transactions.count)
    }

    # save new data
    if daily_transaction_aggregate
      daily_transaction_aggregates_repository.update(daily_transaction_aggregate.id, new_data)
    else
      daily_transaction_aggregates_repository.create(new_data.except('id'))
    end
  end

end
