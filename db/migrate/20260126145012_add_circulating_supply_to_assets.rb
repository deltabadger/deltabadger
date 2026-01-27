class AddCirculatingSupplyToAssets < ActiveRecord::Migration[8.1]
  def change
    add_column :assets, :circulating_supply, :decimal, precision: 30, scale: 8
  end
end
