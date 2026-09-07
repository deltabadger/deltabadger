require 'test_helper'
require 'yaml'

# Reads YAML directly rather than asking I18n. With config.i18n.fallbacks on, I18n.exists? resolves a
# missing key through English and reports true for every locale — so the obvious version of this test
# passes while half the languages render in English.
class Bot::LiquidationLocalesTest < ActiveSupport::TestCase
  KEYS = %w[title sell sell_confirm started market_closed unsupported halted resolve resolve_confirm
            still_open resolved].freeze

  test 'every available_locale carries the sell-position keys in its own bot.<locale>.yml' do
    I18n.available_locales.each do |locale|
      file = Rails.root.join("config/locales/bot.#{locale}.yml")
      assert File.exist?(file), "missing bot locale file for #{locale}"

      data = YAML.load_file(file)[locale.to_s]
      assert data, "bot.#{locale}.yml is missing root '#{locale}:' (malformed file?)"

      KEYS.each do |key|
        assert data.dig('bot', 'liquidation', key), "bot.#{locale}.yml is missing bot.liquidation.#{key}"
      end
    end
  end
end
