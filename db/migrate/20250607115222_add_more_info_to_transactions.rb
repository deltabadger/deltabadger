class AddMoreInfoToTransactions < ActiveRecord::Migration[6.0]
  def change
    rename_column :transactions, :bot_price, :bot_quote_amount
    rename_column :daily_transaction_aggregates, :bot_price, :bot_quote_amount
    rename_column :transactions, :rate, :price
    rename_column :daily_transaction_aggregates, :rate, :price
    add_column :transactions, :side, :integer
    add_column :daily_transaction_aggregates, :side, :integer
    add_column :transactions, :order_type, :integer
    add_column :daily_transaction_aggregates, :order_type, :integer
    add_column :transactions, :filled_percentage, :decimal, precision: 5, scale: 4
    add_column :daily_transaction_aggregates, :filled_percentage, :decimal, precision: 5, scale: 4
    add_column :transactions, :external_status, :integer
    add_column :daily_transaction_aggregates, :external_status, :integer
  end
end
