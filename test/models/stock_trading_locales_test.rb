require 'test_helper'

# Repo convention: no default:, no EN-only strings — every locale carries a
# native translation for GA features (config/locales/CLAUDE.md).
class StockTradingLocalesTest < ActiveSupport::TestCase
  KEYS = %w[
    settings.stocks.title
    settings.stocks.deltabadger_configured
    settings.stocks.disconnected
    settings.stocks.missing_credentials
    settings.stocks.not_refreshing
    bot.setup.stocks_ask_admin
    bot.setup.stocks_activate_cta
  ].freeze

  test 'stock trading keys exist in every available locale' do
    I18n.available_locales.each do |locale|
      KEYS.each do |key|
        # fallback: false — with config.i18n.fallbacks on, a plain exists? is
        # satisfied by the EN string and would never catch a missing locale.
        assert I18n.exists?(key, locale, fallback: false), "missing #{key} in #{locale}"
      end
    end
  end
end
