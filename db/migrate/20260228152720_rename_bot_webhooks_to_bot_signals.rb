class RenameBotWebhooksToBotSignals < ActiveRecord::Migration[8.1]
  def up
    rename_table :bot_webhooks, :bot_signals
    Bot.where(type: 'Bots::Webhook').update_all(type: 'Bots::Signal')
  end

  def down
    rename_table :bot_signals, :bot_webhooks
    Bot.where(type: 'Bots::Signal').update_all(type: 'Bots::Webhook')
  end
end
