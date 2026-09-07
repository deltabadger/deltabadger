require 'test_helper'
require 'yaml'

class Bot::WashSaleLocalesTest < ActiveSupport::TestCase
  KEYS = %w[sentence_html option note].freeze

  test 'every available_locale carries the wash-sale widget keys in its own bot.<locale>.yml' do
    I18n.available_locales.each do |locale|
      data = YAML.load_file(Rails.root.join("config/locales/bot.#{locale}.yml"))[locale.to_s]
      KEYS.each do |key|
        assert data.dig('bot', 'settings', 'wash_sale', key), "bot.#{locale}.yml is missing bot.settings.wash_sale.#{key}"
      end
      %w[locked harvest_hint partial_note].each do |key|
        assert data.dig('bot', 'liquidation', key), "bot.#{locale}.yml is missing bot.liquidation.#{key}"
      end
    end
  end
end
