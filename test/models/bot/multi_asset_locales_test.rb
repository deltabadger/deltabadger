# frozen_string_literal: true

require 'test_helper'
require 'yaml'

# Read the files directly: I18n fallbacks make a missing translation look present in every locale.
class Bot::MultiAssetLocalesTest < ActiveSupport::TestCase
  MULTI_ASSET_KEYS = %w[
    add_asset remove_asset max_assets_reached min_assets
    no_common_exchange removed_from_portfolio allocation_sum normalize normalize_first
  ].freeze
  LIQUIDATION_KEYS = %w[unsupported sell_confirm started].freeze

  test 'every locale carries the multi-asset, liquidation and wizard asset keys' do
    I18n.available_locales.each do |locale|
      bot = locale_data('bot', locale)
      errors = locale_data('errors', locale)
      setup = locale_data('setup', locale)

      MULTI_ASSET_KEYS.each do |key|
        assert bot.dig('bot', 'dca_multi_asset', key).present?,
               "bot.#{locale}.yml is missing bot.dca_multi_asset.#{key}"
      end
      LIQUIDATION_KEYS.each do |key|
        assert bot.dig('bot', 'liquidation', key).present?,
               "bot.#{locale}.yml is missing bot.liquidation.#{key}"
      end
      assert setup.dig('bot', 'setup', 'progress_steps', 'assets').present?,
             "setup.#{locale}.yml is missing bot.setup.progress_steps.assets"
      assert bot.dig('bot', 'exchange_menu', 'locked_while_rebalancing').present?,
             "bot.#{locale}.yml is missing bot.exchange_menu.locked_while_rebalancing"
      assert errors.dig('errors', 'bots', 'multi_asset', 'pair_missing').present?,
             "errors.#{locale}.yml is missing errors.bots.multi_asset.pair_missing"
      # These are I18n interpolation tokens under test, not format strings built by this test.
      # rubocop:disable Style/FormatStringToken
      assert_includes bot.dig('bot', 'dca_multi_asset', 'max_assets_reached'), '%{max}'
      assert_includes bot.dig('bot', 'dca_multi_asset', 'min_assets'), '%{min}'
      # rubocop:enable Style/FormatStringToken
    end

    english = locale_data('bot', :en)
    assert_equal "This combination of assets doesn't exist on any supported exchange.",
                 english.dig('bot', 'dca_multi_asset', 'no_common_exchange')
    assert_equal 'This bot has no removed assets to sell.', english.dig('bot', 'liquidation', 'unsupported')
    assert_equal 'Sell this position? It is closed at market price.', english.dig('bot', 'liquidation', 'sell_confirm')
    assert_equal 'Selling the position.', english.dig('bot', 'liquidation', 'started')
  end

  test 'non-English locales do not silently ship the English strings' do
    english_bot = locale_data('bot', :en)
    english_setup = locale_data('setup', :en)

    (I18n.available_locales - [:en]).each do |locale|
      bot = locale_data('bot', locale)
      errors = locale_data('errors', locale)
      setup = locale_data('setup', locale)

      MULTI_ASSET_KEYS.each do |key|
        next if english_bot.dig('bot', 'dca_multi_asset', key).nil? ||
                bot.dig('bot', 'dca_multi_asset', key).nil?

        assert_not_equal english_bot.dig('bot', 'dca_multi_asset', key),
                         bot.dig('bot', 'dca_multi_asset', key),
                         "#{locale}: bot.dca_multi_asset.#{key} is still English"
      end
      LIQUIDATION_KEYS.each do |key|
        assert_not_equal english_bot.dig('bot', 'liquidation', key),
                         bot.dig('bot', 'liquidation', key),
                         "#{locale}: bot.liquidation.#{key} is still English"
      end
      assert_not_equal english_setup.dig('bot', 'setup', 'progress_steps', 'assets'),
                       setup.dig('bot', 'setup', 'progress_steps', 'assets'),
                       "#{locale}: bot.setup.progress_steps.assets is still English"
      assert_not_equal english_bot.dig('bot', 'exchange_menu', 'locked_while_rebalancing'),
                       bot.dig('bot', 'exchange_menu', 'locked_while_rebalancing'),
                       "#{locale}: bot.exchange_menu.locked_while_rebalancing is still English"
      assert_not_equal locale_data('errors', :en).dig('errors', 'bots', 'multi_asset', 'pair_missing'),
                       errors.dig('errors', 'bots', 'multi_asset', 'pair_missing'),
                       "#{locale}: errors.bots.multi_asset.pair_missing is still English"
    end
  end

  private

  def locale_data(component, locale)
    file = Rails.root.join("config/locales/#{component}.#{locale}.yml")
    assert File.exist?(file), "missing #{component} locale file for #{locale}"
    YAML.load_file(file).fetch(locale.to_s)
  end
end
