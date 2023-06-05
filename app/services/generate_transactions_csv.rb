require 'csv'

class GenerateTransactionsCsv < BaseService
  class GenerateCsv < BaseService
    def call(data)
      return '' if data.empty?

      CSV.generate do |csv|
        csv << data.first.keys
        data.each do |row|
          csv << row.values
        end
      end
    end
  end

  def initialize(
    transactions_repository: TransactionsRepository.new,
    generate_csv: GenerateCsv.new
  )

    @transactions_repository = transactions_repository
    @generate_csv = generate_csv
  end

  def call(bot)
    format_data = begin
      return Presenters::Api::TradingTransaction.new if bot.trading?
      return Presenters::Api::WithdrawalTransaction.new if bot.withdrawal?
      Presenters::Api::WebhookTransaction.new
    end

    transactions = @transactions_repository.for_bot_by_status(bot, status: :success)
    formatted_data = transactions.map { |t| format_data.call(t) }
    @generate_csv.call(formatted_data)
  end
end
