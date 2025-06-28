class RenameExchangeTickerToTicker < ActiveRecord::Migration[6.0]
  def change
    rename_table :exchange_tickers, :tickers
  end
end
