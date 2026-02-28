class AddEnabledToBotWebhooks < ActiveRecord::Migration[8.1]
  def change
    add_column :bot_webhooks, :enabled, :boolean, default: true, null: false
  end
end
