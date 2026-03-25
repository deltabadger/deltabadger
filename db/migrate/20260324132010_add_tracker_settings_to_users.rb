class AddTrackerSettingsToUsers < ActiveRecord::Migration[8.1]
  def change
    add_column :users, :tracker_settings, :json, default: {}
  end
end
