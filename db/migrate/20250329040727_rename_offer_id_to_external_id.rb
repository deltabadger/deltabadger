class RenameOfferIdToExternalId < ActiveRecord::Migration[6.0]
  def change
    rename_column :transactions, :offer_id, :external_id
    rename_column :daily_transaction_aggregates, :offer_id, :external_id

    # find duplicated external ids
    external_ids = Transaction.pluck(:external_id).compact
    duplicated_external_ids = external_ids.select { |id| external_ids.count(id) > 1 }
    puts "Duplicated external ids: #{duplicated_external_ids.inspect}"

    # update transactions with duplicated external ids
    duplicated_transactions = {}
    Transaction.where(external_id: duplicated_external_ids).find_each do |transaction|
      if duplicated_transactions[transaction.external_id].present?
        if transaction.bot_id == duplicated_transactions[transaction.external_id].bot_id &&
           transaction.rate == duplicated_transactions[transaction.external_id].rate &&
           transaction.amount == duplicated_transactions[transaction.external_id].amount &&
           transaction.status == duplicated_transactions[transaction.external_id].status &&
           transaction.error_messages == duplicated_transactions[transaction.external_id].error_messages &&
           transaction.bot_price == duplicated_transactions[transaction.external_id].bot_price &&
           transaction.bot_interval == duplicated_transactions[transaction.external_id].bot_interval &&
           transaction.transaction_type == duplicated_transactions[transaction.external_id].transaction_type &&
           transaction.called_bot_type == duplicated_transactions[transaction.external_id].called_bot_type &&
           transaction.base == duplicated_transactions[transaction.external_id].base &&
           transaction.quote == duplicated_transactions[transaction.external_id].quote &&
           transaction.exchange_id == duplicated_transactions[transaction.external_id].exchange_id
          transaction.destroy
        else
          puts "Inconsistent transaction #{transaction.id} with external id #{transaction.external_id}. Transactions doesn't match:"
          puts "transaction: #{transaction.attributes.inspect}"
          puts "duplicated_transaction: #{duplicated_transactions[transaction.external_id].attributes.inspect}"
          raise
        end
      else
        duplicated_transactions[transaction.external_id] = transaction
      end
    end

    add_index :transactions, :external_id, unique: true
  end
end
