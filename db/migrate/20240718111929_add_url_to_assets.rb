class AddUrlToAssets < ActiveRecord::Migration[6.0]
  def change
    add_column :assets, :url, :string
  end
end
