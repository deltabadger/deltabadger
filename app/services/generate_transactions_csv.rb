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
    generate_csv: GenerateCsv.new
  )
    @generate_csv = generate_csv
  end

  def call(bot)
    format_data = if bot.basic?
                    Presenters::Api::TradingTransaction.new
                  elsif bot.withdrawal?
                    Presenters::Api::WithdrawalTransaction.new
                  else
                    Presenters::Api::WebhookTransaction.new
                  end

    transactions = Transaction.for_bot_by_status(bot, status: :success)
    formatted_data = transactions.map { |t| format_data.call(t) }
    @generate_csv.call(formatted_data)
  end
end
