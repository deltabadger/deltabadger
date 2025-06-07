class AddMoreInfoToTransactions < ActiveRecord::Migration[6.0]
  def change
    rename_column :transactions, :bot_price, :bot_quote_amount
    rename_column :daily_transaction_aggregates, :bot_price, :bot_quote_amount
  end
end
