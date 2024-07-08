class ChangeCategoryTypeInAsset < ActiveRecord::Migration[6.0]
  def up
    add_column :assets, :category_string, :string
    category_mapping = {
      0 => 'Cryptocurrency',
      1 => 'Common Stock',
      2 => 'Index',
      3 => 'Bond'
    }
    Asset.reset_column_information
    Asset.find_each do |asset|
      asset.update(category_string: category_mapping[asset.category])
    end
    remove_column :assets, :category
    rename_column :assets, :category_string, :category
  end

  def down
    add_column :assets, :category_integer, :integer
    category_mapping = {
      'Cryptocurrency' => 0,
      'Common Stock' => 1,
      'Index' => 2,
      'Bond' => 3
    }
    Asset.reset_column_information
    Asset.find_each do |asset|
      asset.update(category_integer: category_mapping[asset.category])
    end
    remove_column :assets, :category
    rename_column :assets, :category_integer, :category
  end
end
