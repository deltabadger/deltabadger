class AddHideBalancesToUsers < ActiveRecord::Migration[8.1]
  def change
    add_column :users, :hide_balances, :boolean, null: false, default: false
  end
end
