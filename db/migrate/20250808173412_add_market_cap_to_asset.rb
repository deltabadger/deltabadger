class AddMarketCapToAsset < ActiveRecord::Migration[6.0]
  def change
    add_column :assets, :market_cap, :bigint
  end
end
