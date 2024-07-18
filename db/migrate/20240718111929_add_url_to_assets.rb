class AddUrlToAssets < ActiveRecord::Migration[6.0]
  def change
    add_column :assets, :url, :string
    add_column :assets, :country, :string
    add_column :assets, :exchange, :string
  end
end
