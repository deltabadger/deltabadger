class AddTypeToBots < ActiveRecord::Migration[6.0]
  def up
    add_column :bots, :type, :string

    Bot.find_each do |bot|
      case bot.bot_type
      when 0
        bot.update_column(:type, 'DcaBot')
      when 1
        bot.update_column(:type, 'WithdrawalBot')
      when 2
        bot.update_column(:type, 'WebhookBot')
      when 3
        bot.update_column(:type, 'BarbellBot')
      end
    end

    remove_column :bots, :bot_type
  end

  def down
    add_column :bots, :bot_type, :integer

    Bot.find_each do |bot|
      case bot.type
      when 'DcaBot'
        bot.update_column(:bot_type, 0)
      when 'WithdrawalBot'
        bot.update_column(:bot_type, 1)
      when 'WebhookBot'
        bot.update_column(:bot_type, 2)
      when 'BarbellBot'
        bot.update_column(:bot_type, 3)
      end
    end

    remove_column :bots, :type
  end
end
