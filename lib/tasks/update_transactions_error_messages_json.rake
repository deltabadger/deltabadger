desc 'rake task to update transactions error_messages_json'
task update_transactions_error_messages_json: :environment do
  Transaction.find_each do |transaction|
    next if transaction.error_messages == []

    puts "Updating transaction #{transaction.id}"
    transaction.update!(error_messages_json: transaction.error_messages)
  end
end
