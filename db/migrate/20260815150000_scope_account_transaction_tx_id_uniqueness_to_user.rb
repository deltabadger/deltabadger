class ScopeAccountTransactionTxIdUniquenessToUser < ActiveRecord::Migration[8.1]
  def up
    # The new key is a superset of the old one, so it is strictly weaker and cannot reject an existing row.
    remove_index :account_transactions, name: :index_account_transactions_on_exchange_id_and_tx_id
    add_index :account_transactions, %i[user_id exchange_id tx_id],
              name: :index_account_transactions_on_user_exchange_tx_id,
              unique: true, where: 'tx_id IS NOT NULL'
  end

  def down
    remove_index :account_transactions, column: %i[user_id exchange_id tx_id]
    add_index :account_transactions, %i[exchange_id tx_id], unique: true, where: 'tx_id IS NOT NULL'
  end
end
