desc 'rake task to update transactions decimals'
task update_transactions_decimals: :environment do
  DailyTransactionAggregate.find_each do |daily_transaction_aggregate|
    puts "Updating daily_transaction_aggregate #{daily_transaction_aggregate.id}"
    ActiveRecord::Base.transaction do
      previous_bot_quote_amount = daily_transaction_aggregate.bot_quote_amount
      daily_transaction_aggregate.update!(
        price: daily_transaction_aggregate.price&.round(18),
        amount: daily_transaction_aggregate.amount.round(18),
        bot_quote_amount: 0,
        total_amount: daily_transaction_aggregate.total_amount.round(18),
        total_value: daily_transaction_aggregate.total_value.round(18),
        total_invested: daily_transaction_aggregate.total_invested.round(18)
      )
      daily_transaction_aggregate.update!(bot_quote_amount: previous_bot_quote_amount)
    end
  end

  Transaction.find_each do |transaction|
    puts "Updating transaction #{transaction.id}"
    ActiveRecord::Base.transaction do
      previous_bot_quote_amount = transaction.bot_quote_amount
      transaction.update!(
        price: transaction.price&.round(18),
        amount: transaction.amount&.round(18),
        bot_quote_amount: 0
      )
      transaction.update!(bot_quote_amount: previous_bot_quote_amount)
    end
  end
end
