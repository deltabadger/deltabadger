class AddTypeToBots < ActiveRecord::Migration[6.0]
  def up
    Rails.logger.info 'Adding type to bots'
    puts 'Adding type to bots'
    add_column :bots, :type, :string

    Bot.find_each do |bot|
      case bot.bot_type
      when 0
        Rails.logger.info "Updating bot #{bot.id} with type DcaBot"
        puts "Updating bot #{bot.id} with type DcaBot"
        bot.update_column(:type, 'DcaBot')
      when 1
        Rails.logger.info "Updating bot #{bot.id} with type WithdrawalBot"
        puts "Updating bot #{bot.id} with type WithdrawalBot"
        bot.update_column(:type, 'WithdrawalBot')
      when 2
        Rails.logger.info "Updating bot #{bot.id} with type WebhookBot"
        puts "Updating bot #{bot.id} with type WebhookBot"
        bot.update_column(:type, 'WebhookBot')
      when 3
        Rails.logger.info "Updating bot #{bot.id} with type BarbellBot"
        puts "Updating bot #{bot.id} with type BarbellBot"
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
