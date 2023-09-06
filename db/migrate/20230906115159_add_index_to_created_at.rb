class AddIndexToCreatedAt < ActiveRecord::Migration[6.0] # or your current Rails version
  def change
    add_index :transactions, :created_at
    add_index :daily_transaction_aggregates, :created_at
  end
end
