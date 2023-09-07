class AddTotalAmountTotalValueTotalInvestedToDailyTransactionAggregates < ActiveRecord::Migration[6.0]
  def change
    add_column :daily_transaction_aggregates, :total_amount, :decimal, precision: 20, scale: 10, default: "0.0", null: false
    add_column :daily_transaction_aggregates, :total_value, :decimal, precision: 20, scale: 10, default: "0.0", null: false
    add_column :daily_transaction_aggregates, :total_invested, :decimal, precision: 20, scale: 10, default: "0.0", null: false
  end
end
