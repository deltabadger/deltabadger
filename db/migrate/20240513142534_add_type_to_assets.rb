class AddTypeToAssets < ActiveRecord::Migration[6.0]
  def change
    add_column :assets, :type, :integer
  end
end
