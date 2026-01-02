class RemoveAhoy < ActiveRecord::Migration[6.0]
  def change
    drop_table :ahoy_messages
    drop_table :ahoy_clicks
    drop_table :ahoy_opens
  end
end
