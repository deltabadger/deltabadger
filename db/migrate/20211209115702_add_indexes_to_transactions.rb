class AddIndexesToTransactions < ActiveRecord::Migration[5.2]
  def change
    add_index :transactions, [:bot_id, :transaction_type, :created_at], name: 'index_bot_type_created_at'
    add_index :transactions, [:bot_id, :created_at]
    add_index :transactions, [:bot_id, :status, :created_at]
  end
end
