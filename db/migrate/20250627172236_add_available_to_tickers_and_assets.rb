class AddAvailableToTickersAndAssets < ActiveRecord::Migration[6.0]
  def change
    add_column :exchange_tickers, :available, :boolean, default: true
    add_column :exchange_assets, :available, :boolean, default: true
  end
end
