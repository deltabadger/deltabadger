class Transaction < ApplicationRecord
  belongs_to :bot
  enum currency: %i[USD EUR PLN]
  enum status: %i[success failure skipped]

  after_create :set_transaction_daily

  validates :bot, presence: true

  def get_errors
    JSON.parse(error_messages)
  end

  def price
    return 0.0 unless rate.present?

    amount * rate
  end

  private

  def set_transaction_daily
    return unless success?

    transactions_repository = TransactionsRepository.new
    transactions_daily_repository = TransactionsDailyRepository.new
    transaction_daily = transactions_daily_repository.today_for_bot(bot).first
    return transactions_daily_repository.create(attributes.except('id')) unless transaction_daily

    bot_transactions = transactions_repository.today_for_bot(bot)
    transaction_daily_new_data = {rate: bot_transactions.sum(&:rate)/bot_transactions.count, amount: bot_transactions.sum(&:amount)}
    transactions_daily_repository.update(transaction_daily.id, transaction_daily_new_data)
  end
end
