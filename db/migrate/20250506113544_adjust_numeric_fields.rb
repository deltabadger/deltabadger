class AdjustNumericFields < ActiveRecord::Migration[6.0]
  def up
    change_column :daily_transaction_aggregates, :bot_price, :decimal, precision: nil, scale: nil
    change_column :daily_transaction_aggregates, :total_amount, :decimal, precision: nil, scale: nil
    change_column :daily_transaction_aggregates, :total_value, :decimal, precision: nil, scale: nil
    change_column :daily_transaction_aggregates, :total_invested, :decimal, precision: nil, scale: nil

    change_column :portfolio_assets, :allocation, :decimal, precision: 5, scale: 4, default: 0.0, null: false

    change_column :portfolios, :risk_free_rate, :decimal, precision: 5, scale: 4, default: 0.0, null: false

    change_column :transactions, :bot_price, :decimal, precision: nil, scale: nil
  end

  def down
    change_column :daily_transaction_aggregates, :bot_price, :decimal, precision: 20, scale: 10
    change_column :daily_transaction_aggregates, :total_amount, :decimal, precision: 20, scale: 10
    change_column :daily_transaction_aggregates, :total_value, :decimal, precision: 20, scale: 10
    change_column :daily_transaction_aggregates, :total_invested, :decimal, precision: 20, scale: 10

    change_column :portfolio_assets, :allocation, :float, default: 0.0, null: false

    change_column :portfolios, :risk_free_rate, :float, default: 0.0, null: false

    change_column :transactions, :bot_price, :decimal, precision: 20, scale: 10
  end
end
