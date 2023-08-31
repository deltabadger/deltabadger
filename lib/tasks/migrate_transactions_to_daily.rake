desc 'rake task to transfer data from the transactions table to the transactions_daily table'
task migrate_transactions_to_daily: :environment do
  p "! "*5+'Start script to create TransactionsDaily records'+" !"*5

  transactions_daily_repository = TransactionsDailyRepository.new

  transactions_grouped = Transaction.success.group_by { |transaction| [transaction.bot_id, transaction.created_at.to_date] }

  p "#{transactions_grouped.count} records should be created"

  transactions_grouped.each_with_index do |(_, transactions), index|
    p "Start of creating record ##{index+1}"

    transaction_daily_data = transactions.last.attributes.except('id')
    daily_rate = transactions.sum(&:rate)/transactions.count
    daily_amount = transactions.sum(&:amount)
    transaction_daily_data.merge!("rate" => daily_rate, "amount" => daily_amount)

    transactions_daily_repository.create(transaction_daily_data)

    p "Record ##{index+1} was successfully created"
  end

  p "! "*5+'The script to create TransactionsDaily records has completed successfully'+" !"*5
end
