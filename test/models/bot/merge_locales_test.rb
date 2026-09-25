# frozen_string_literal: true

require 'test_helper'
require 'yaml'

# Merge's and Split's strings. Read the files directly: I18n fallbacks make a missing translation look present in every locale.
class Bot::MergeLocalesTest < ActiveSupport::TestCase
  # These are I18n interpolation tokens under test, not format strings built by this test.
  # rubocop:disable Style/FormatStringToken
  KEYS = {
    'base' => {
      %w[button merge] => nil,
      %w[button confirm] => nil,
      %w[bot_activity events merged] => '%{labels}',
      %w[button split] => nil,
      %w[bot_activity events split] => '%{label}'
    },
    'bot' => {
      %w[bot dca_multi_asset too_many_assets] => '%{count}',
      %w[bot merge title] => '%{count}',
      %w[bot merge pick_hint] => nil,
      %w[bot merge explanation_html] => '%{exchange}',
      %w[bot merge deleted_note] => nil,
      %w[bot merge success] => nil,
      %w[bot merge no_partner] => '%{quote}',
      %w[bot merge no_shared_exchange] => '%{quote}',
      %w[bot merge other_exchange] => '%{exchange}',
      %w[bot merge assets_stay] => nil,
      %w[bot split explanation] => nil,
      %w[bot split proceeds] => '%{amount}',
      %w[bot split kept] => nil,
      %w[bot split success] => nil
    },
    'errors' => {
      %w[errors bots merge missing] => nil,
      %w[errors bots merge too_few] => nil,
      %w[errors bots merge unavailable] => '%{label}',
      %w[errors bots merge no_common_exchange] => nil,
      %w[errors bots merge open_orders] => '%{exchange}',
      %w[errors bots merge external_sales] => '%{label}',
      %w[errors bots merge interleaved] => nil,
      %w[errors bots merge quote] => nil,
      %w[errors bots merge nothing_to_buy] => nil,
      %w[errors bots split missing] => nil,
      %w[errors bots split busy] => '%{exchange}',
      %w[errors bots split none] => nil,
      %w[errors bots split unavailable] => '%{label}',
      %w[errors bots split reinvesting] => '%{label}',
      %w[errors bots split proceeds] => '%{label}',
      %w[errors bots split external_sales] => '%{label}',
      %w[errors bots split unpriced_sales] => '%{label}',
      %w[errors bots split nothing_to_buy] => '%{label}'
    }
  }.freeze

  ENGLISH = {
    %w[button merge] => 'Merge',
    %w[button confirm] => 'Confirm',
    %w[bot dca_multi_asset too_many_assets] => 'Too many assets. Remove %{count}.',
    %w[bot merge title] => 'Merge %{count} bots',
    %w[bot merge pick_hint] => 'Select bots.',
    %w[bot merge explanation_html] => 'Their purchase history will be merged into one new bot investing %{quote} on %{exchange} into:',
    %w[bot merge deleted_note] => 'The original bots will be deleted.',
    %w[errors bots merge too_few] => 'Pick at least two bots.'
  }.freeze
  LABEL_TOKEN = '%{label}'
  # rubocop:enable Style/FormatStringToken

  test 'every locale carries every merge key with its interpolation tokens' do
    I18n.available_locales.each do |locale|
      KEYS.each do |component, keys|
        data = locale_data(component, locale)
        keys.each do |path, token|
          value = data.dig(*path)
          assert value.present?, "#{component}.#{locale}.yml is missing #{path.join('.')}"
          assert_includes value, token, "#{component}.#{locale}.yml #{path.join('.')} lost #{token}" if token
        end
      end
    end
  end

  test 'the interleaving refusal names no bot: it is about the combination, not one of them' do
    I18n.available_locales.each do |locale|
      assert_not_includes locale_data('errors', locale).dig('errors', 'bots', 'merge', 'interleaved'), LABEL_TOKEN,
                          "errors.#{locale}.yml"
    end
  end

  test 'the English strings are what the design says' do
    ENGLISH.each do |path, expected|
      component = %w[button bot_activity].include?(path.first) ? 'base' : path.first
      assert_equal expected, locale_data(component, :en).dig(*path)
    end
    assert_nil locale_data('errors', :en).dig('errors', 'bots', 'merge', 'exchange'), 'replaced by no_common_exchange'
  end

  test 'non-English locales do not silently ship the English strings' do
    english = KEYS.transform_values { |keys| keys.keys.to_h { |path| [path, nil] } }
    english.each_key { |component| english[component].each_key { |path| english[component][path] = locale_data(component, :en).dig(*path) } }

    (I18n.available_locales - [:en]).each do |locale|
      KEYS.each do |component, keys|
        data = locale_data(component, locale)
        keys.each_key do |path|
          assert_not_equal english[component][path], data.dig(*path),
                           "#{component}.#{locale}.yml ships the English #{path.join('.')}"
        end
      end
    end
  end

  private

  def locale_data(component, locale)
    YAML.load_file(Rails.root.join("config/locales/#{component}.#{locale}.yml")).fetch(locale.to_s)
  end
end
