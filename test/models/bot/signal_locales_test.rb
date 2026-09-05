# frozen_string_literal: true

require 'test_helper'
require 'yaml'

# Read the files directly: I18n fallbacks make a missing translation look present in every locale.
class Bot::SignalLocalesTest < ActiveSupport::TestCase
  test 'every locale can say why a signal did nothing, and when a webhook was last called' do
    I18n.available_locales.each do |locale|
      events = YAML.load_file(Rails.root.join("config/locales/base.#{locale}.yml"))
                   .dig(locale.to_s, 'bot_activity', 'events') || {}
      signal = YAML.load_file(Rails.root.join("config/locales/bot.#{locale}.yml"))
                   .dig(locale.to_s, 'bot', 'signal') || {}

      %w[signal_market_closed signal_api_key_pending signal_expired signal_ignored].each do |event|
        assert events[event].present?, "base.#{locale}.yml is missing bot_activity.events.#{event}"
      end
      assert signal['never_triggered'].present?, "bot.#{locale}.yml is missing bot.signal.never_triggered"
      assert signal['last_triggered'].present?, "bot.#{locale}.yml is missing bot.signal.last_triggered"
      # An I18n interpolation token under test, not a format string built by this test.
      assert_includes signal['last_triggered'], '%{time}', # rubocop:disable Style/FormatStringToken
                      "bot.#{locale}.yml last_triggered drops the time"
    end
  end
end
