class AddNameToAssets < ActiveRecord::Migration[6.0]
  def change
    add_column :assets, :name, :string
  end
end
