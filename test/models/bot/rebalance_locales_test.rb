require 'test_helper'
require 'yaml'

# Reads YAML directly rather than asking I18n. With config.i18n.fallbacks on, I18n.exists? resolves a
# missing key through English and reports true for every locale — so the obvious version of this test
# passes while half the languages render in English.
class Bot::RebalanceLocalesTest < ActiveSupport::TestCase
  KEYS = %w[sentence_html note drift below_minimum paused resume resume_confirm still_open resolved].freeze

  # Activity rows with no explicit message render through bot_activity.events.<event>, so every
  # event the rebalance leg emits needs a key or the feed shows raw translation-missing text.
  # rebalance_sell_placed / rebalance_buy_placed are built by interpolation
  # ("rebalance_#{side}_placed"), which no grep for a literal event name would find.
  EVENTS = %w[
    rebalance_sell_placed rebalance_buy_placed rebalance_dust
    rebalance_ambiguous rebalance_manually_resolved dca_skipped_rebalance_pending
  ].freeze

  test 'every available_locale carries the rebalance widget keys in its own bot.<locale>.yml' do
    I18n.available_locales.each do |locale|
      file = Rails.root.join("config/locales/bot.#{locale}.yml")
      assert File.exist?(file), "missing bot locale file for #{locale}"

      data = YAML.load_file(file)[locale.to_s]
      assert data, "bot.#{locale}.yml is missing root '#{locale}:' (malformed file?)"

      KEYS.each do |key|
        assert data.dig('bot', 'settings', 'rebalance', key),
               "bot.#{locale}.yml is missing bot.settings.rebalance.#{key}"
      end
    end
  end

  test 'every available_locale carries the rebalance activity events in its own base.<locale>.yml' do
    I18n.available_locales.each do |locale|
      file = Rails.root.join("config/locales/base.#{locale}.yml")
      assert File.exist?(file), "missing base locale file for #{locale}"

      data = YAML.load_file(file)[locale.to_s]
      EVENTS.each do |event|
        assert data.dig('bot_activity', 'events', event).present?,
               "base.#{locale}.yml is missing bot_activity.events.#{event}"
      end
    end
  end

  test 'no locale silently ships the English string' do
    # A key copied verbatim from English passes a presence check while reading as untranslated.
    english = YAML.load_file(Rails.root.join('config/locales/bot.en.yml'))['en']
    english_events = YAML.load_file(Rails.root.join('config/locales/base.en.yml'))['en']

    (I18n.available_locales - [:en]).each do |locale|
      data = YAML.load_file(Rails.root.join("config/locales/bot.#{locale}.yml"))[locale.to_s]
      events = YAML.load_file(Rails.root.join("config/locales/base.#{locale}.yml"))[locale.to_s]

      KEYS.each do |key|
        assert_not_equal english.dig('bot', 'settings', 'rebalance', key),
                         data.dig('bot', 'settings', 'rebalance', key),
                         "#{locale}: bot.settings.rebalance.#{key} is still the English string"
      end
      EVENTS.each do |event|
        assert_not_equal english_events.dig('bot_activity', 'events', event),
                         events.dig('bot_activity', 'events', event),
                         "#{locale}: bot_activity.events.#{event} is still the English string"
      end
    end
  end

  # These are I18n interpolation tokens being asserted on as literal text, not format strings this
  # test is building — the annotated-token style the cop wants would break the assertion.
  # rubocop:disable Style/FormatStringToken
  test 'the interpolations every locale must preserve are present' do
    # A translation that drops %{value_html} renders a rule with no input box in it.
    I18n.available_locales.each do |locale|
      data = YAML.load_file(Rails.root.join("config/locales/bot.#{locale}.yml"))[locale.to_s]
      rebalance = data.dig('bot', 'settings', 'rebalance')

      assert_includes rebalance['sentence_html'], '%{value_html}', "#{locale}: sentence lost its input"
      assert_includes rebalance['drift'], '%{drift}', "#{locale}: drift readout lost its number"
      assert_includes rebalance['still_open'], '%{order_id}', "#{locale}: still_open lost the order id"
    end
  end
  # rubocop:enable Style/FormatStringToken
end
