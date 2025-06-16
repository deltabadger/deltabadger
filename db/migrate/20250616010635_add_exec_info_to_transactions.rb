class AddExecInfoToTransactions < ActiveRecord::Migration[6.0]
  def change
    add_column :transactions, :amount_exec, :decimal
    add_column :transactions, :quote_amount_exec, :decimal
    add_column :daily_transaction_aggregates, :amount_exec, :decimal
    add_column :daily_transaction_aggregates, :quote_amount_exec, :decimal
    remove_column :transactions, :filled_percentage
    remove_column :daily_transaction_aggregates, :filled_percentage
  end
end
