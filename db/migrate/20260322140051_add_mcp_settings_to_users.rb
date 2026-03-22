class AddMCPSettingsToUsers < ActiveRecord::Migration[8.1]
  def change
    add_column :users, :mcp_settings, :json, default: {}
  end
end
