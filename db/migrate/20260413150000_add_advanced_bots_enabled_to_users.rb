class AddAdvancedBotsEnabledToUsers < ActiveRecord::Migration[8.1]
  def change
    add_column :users, :advanced_bots_enabled, :boolean, default: false, null: false
  end
end
