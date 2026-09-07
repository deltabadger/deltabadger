class AddWashSaleSettingsToUsers < ActiveRecord::Migration[8.1]
  class MigrationUser < ApplicationRecord
    self.table_name = 'users'
  end

  class MigrationBot < ApplicationRecord
    self.table_name = 'bots'
    # `bots.type` carries Bots::DcaIndex &c., which are not subclasses of THIS class — without
    # this every find raises ActiveRecord::SubclassNotFound.
    self.inheritance_column = nil
  end

  def up
    add_column :users, :wash_sale_enabled, :boolean, default: false, null: false
    add_column :users, :wash_sale_jurisdiction, :string
    # When the account was asked, so the first-start prompt asks once and never nags again.
    add_column :users, :wash_sale_prompted_at, :datetime

    say_with_time 'moving wash-sale settings from bots to their owners' do
      boolean = ActiveModel::Type::Boolean.new
      MigrationBot.where.not(user_id: nil).order(:id).each do |bot|
        settings = bot.settings.is_a?(Hash) ? bot.settings : {}
        next unless boolean.cast(settings['wash_sale_enabled'])

        user = MigrationUser.find_by(id: bot.user_id)
        # The first bot that had it on wins; having answered on a bot counts as having been asked.
        next if user.nil? || user.wash_sale_enabled

        user.update_columns(wash_sale_enabled: true,
                            wash_sale_jurisdiction: settings['wash_sale_jurisdiction'],
                            wash_sale_prompted_at: Time.current)
      end

      MigrationBot.find_each do |bot|
        next unless bot.settings.is_a?(Hash) && (bot.settings.keys & %w[wash_sale_enabled wash_sale_jurisdiction]).any?

        bot.update_columns(settings: bot.settings.except('wash_sale_enabled', 'wash_sale_jurisdiction'))
      end
    end
  end

  # Reversible for real: the up step DELETED the per-bot settings, so dropping the user columns
  # without writing them back would leave the fleet with the rule silently off everywhere.
  def down
    say_with_time 'moving wash-sale settings back onto the composition bots' do
      MigrationUser.where(wash_sale_enabled: true).find_each do |user|
        MigrationBot.where(user_id: user.id,
                           type: %w[Bots::DcaIndex Bots::DcaMultiAsset]).find_each do |bot|
          settings = bot.settings.is_a?(Hash) ? bot.settings : {}
          bot.update_columns(settings: settings.merge(
            'wash_sale_enabled' => true,
            'wash_sale_jurisdiction' => user.wash_sale_jurisdiction
          ))
        end
      end
    end

    remove_column :users, :wash_sale_enabled
    remove_column :users, :wash_sale_jurisdiction
    remove_column :users, :wash_sale_prompted_at
  end
end
