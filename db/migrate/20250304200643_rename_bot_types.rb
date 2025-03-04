class RenameBotTypes < ActiveRecord::Migration[6.0]
  def up
    Bot.where(type: 'BarbellBot').update_all(type: 'Bots::Barbell')
    Bot.where(type: 'DcaBot').update_all(type: 'Bots::Basic')
    Bot.where(type: 'WebhookBot').update_all(type: 'Bots::Webhook')
    Bot.where(type: 'WithdrawalBot').update_all(type: 'Bots::Withdrawal')
  end

  def down
    Bot.where(type: 'Bots::Barbell').update_all(type: 'BarbellBot')
    Bot.where(type: 'Bots::Basic').update_all(type: 'DcaBot')
    Bot.where(type: 'Bots::Webhook').update_all(type: 'WebhookBot')
    Bot.where(type: 'Bots::Withdrawal').update_all(type: 'WithdrawalBot')
  end
end
