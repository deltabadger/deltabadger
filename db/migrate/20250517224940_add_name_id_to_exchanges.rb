class AddNameIdToExchanges < ActiveRecord::Migration[6.0]
  def change
    add_column :exchanges, :name_id, :string, unique: true
    add_index :exchanges, :name_id, unique: true
  end
end
