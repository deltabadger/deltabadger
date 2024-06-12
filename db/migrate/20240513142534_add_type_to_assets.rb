class AddTypeToAssets < ActiveRecord::Migration[6.0]
  def change
    add_column :assets, :category, :integer
  end
end
