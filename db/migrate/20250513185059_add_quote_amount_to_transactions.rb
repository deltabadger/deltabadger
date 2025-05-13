class AddQuoteAmountToTransactions < ActiveRecord::Migration[6.0]
  def change
    add_column :transactions, :quote_amount, :decimal, null: true
    add_column :daily_transaction_aggregates, :quote_amount, :decimal, null: true
  end
end
