class AddAthToExchangeTickers < ActiveRecord::Migration[6.0]
  def change
    add_column :exchange_tickers, :ath, :decimal
    add_column :exchange_tickers, :ath_updated_at, :datetime
  end
end
