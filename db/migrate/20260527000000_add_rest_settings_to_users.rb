class AddRestSettingsToUsers < ActiveRecord::Migration[8.1]
  def change
    add_column :users, :rest_settings, :json, default: {}
  end
end
