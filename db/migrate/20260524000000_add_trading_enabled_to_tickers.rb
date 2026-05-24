class AddTradingEnabledToTickers < ActiveRecord::Migration[8.1]
  def change
    add_column :tickers, :trading_enabled, :boolean, null: false, default: true
  end
end
