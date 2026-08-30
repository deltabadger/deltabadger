# frozen_string_literal: true

require 'test_helper'
require 'yaml'

# Read the files directly: I18n fallbacks make a missing translation look present in every locale.
class Bot::AssetSplitLocalesTest < ActiveSupport::TestCase
  test 'every locale can say a split happened, with and without a ratio' do
    I18n.available_locales.each do |locale|
      events = YAML.load_file(Rails.root.join("config/locales/base.#{locale}.yml"))
                   .dig(locale.to_s, 'bot_activity', 'events')

      with_ratio = events&.dig('asset_split')
      without_ratio = events&.dig('asset_split_unknown_ratio')

      assert with_ratio.present?, "base.#{locale}.yml is missing bot_activity.events.asset_split"
      assert without_ratio.present?,
             "base.#{locale}.yml is missing bot_activity.events.asset_split_unknown_ratio"
      # These are I18n interpolation tokens under test, not format strings built by this test.
      # rubocop:disable Style/FormatStringToken
      assert_includes with_ratio, '%{base}', "base.#{locale}.yml asset_split drops the symbol"
      assert_includes with_ratio, '%{ratio}', "base.#{locale}.yml asset_split drops the ratio"
      assert_includes without_ratio, '%{base}',
                      "base.#{locale}.yml asset_split_unknown_ratio drops the symbol"
      assert_not_includes without_ratio, '%{ratio}',
                          "base.#{locale}.yml asset_split_unknown_ratio has no ratio to name"
      assert events['dca_skipped_restatement'].present?,
             "base.#{locale}.yml is missing bot_activity.events.dca_skipped_restatement"
      # rubocop:enable Style/FormatStringToken
    end
  end
end
