class AddLinkedTransactionToAccountTransactions < ActiveRecord::Migration[8.1]
  def change
    add_column :account_transactions, :linked_transaction_id, :integer
    add_index :account_transactions, :linked_transaction_id, unique: true
  end
end
