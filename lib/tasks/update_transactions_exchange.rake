desc 'rake task to update transactions exchange'
task update_transactions_exchange: :environment do
  Transaction.find_each do |transaction|
    next if transaction.exchange_id.present?

    puts "Updating transaction #{transaction.id}"
    transaction.update!(exchange_id: transaction.bot.exchange_id)
  end
end
