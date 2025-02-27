class ChangeBotsSettingsDefault < ActiveRecord::Migration[6.0]
  def change
    change_column_default :bots, :settings, from: "", to: {}
  end
end
