class AddDataToExchanges < ActiveRecord::Migration[6.0]
  def change
    add_column :exchanges, :url, :string
    add_column :exchanges, :color, :string
    add_column :exchanges, :external_id, :string
  end
end
