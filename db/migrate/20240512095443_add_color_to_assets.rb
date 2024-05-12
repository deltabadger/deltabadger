class AddColorToAssets < ActiveRecord::Migration[6.0]
  def change
    add_column :assets, :color, :string
  end
end
