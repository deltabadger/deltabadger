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
    generate_csv: GenerateCsv.new,
    format_output: Presenters::Api::Transaction.new
  )

    @transactions_repository = transactions_repository
    @generate_csv = generate_csv
    @format_output = format_output
  end

  def call(bot)
    transactions = @transactions_repository.for_bot(bot)
    formatted_data = transactions.map { |t| @format_output.call(t) }
    @generate_csv.call(formatted_data)
  end
end
