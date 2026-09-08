require 'test_helper'
require 'yaml'

# The widget renders from a fallback-free lookup, so a locale missing a key ships a raw key string to
# a real user. The setting is account-scoped now, hence base.<locale>.yml rather than bot.<locale>.yml.
class Bot::WashSaleLocalesTest < ActiveSupport::TestCase
  WASH_SALE_KEYS = %w[title sentence_html option note confirm locked_title locked_none locked_entry].freeze

  test 'every available_locale carries the wash-sale widget keys in its own base.<locale>.yml' do
    I18n.available_locales.each do |locale|
      data = YAML.load_file(Rails.root.join("config/locales/base.#{locale}.yml"))[locale.to_s]
      WASH_SALE_KEYS.each do |key|
        assert data.dig('settings', 'wash_sale', key),
               "base.#{locale}.yml is missing settings.wash_sale.#{key}"
      end
    end
  end

  # Activity rows persist. The composition leg no longer writes this event, but the rows it already
  # wrote render through t("bot_activity.events.\#{event}") forever.
  test 'the retired skip event is still translatable, because its rows still exist' do
    I18n.available_locales.each do |locale|
      data = YAML.load_file(Rails.root.join("config/locales/base.#{locale}.yml"))[locale.to_s]
      assert data.dig('bot_activity', 'events', 'dca_skipped_wash_sale'),
             "base.#{locale}.yml is missing bot_activity.events.dca_skipped_wash_sale"
    end
  end

  test 'every available_locale carries the liquidation keys in its own bot.<locale>.yml' do
    I18n.available_locales.each do |locale|
      data = YAML.load_file(Rails.root.join("config/locales/bot.#{locale}.yml"))[locale.to_s]
      %w[locked harvest_hint partial_note].each do |key|
        assert data.dig('bot', 'liquidation', key), "bot.#{locale}.yml is missing bot.liquidation.#{key}"
      end
      %w[table_title settings_link from_ledger].each do |key|
        assert data.dig('bot', 'wash_sale', key), "bot.#{locale}.yml is missing bot.wash_sale.#{key}"
      end
    end
  end
end
