# frozen_string_literal: true

require 'test_helper'

# A condition sentence takes its subject as markup, because with several members it is a select.
# With one member the subject is the asset's symbol out of the database, so it must arrive escaped:
# no shipped sentence prints it today, but one edited translation would.
class ConditionSubjectTest < ActionView::TestCase
  SENTENCES = {
    'price_limit' => %i[extra_price_limit sentence_html base_html],
    'price_drop_limit' => %i[extra_price_drop_limit mode_sentence_html base_html],
    'moving_average_limit' => %i[extra_moving_average_limit sentence_html ticker_html],
    'indicator_limit' => %i[extra_indicator_limit sentence_html ticker_html]
  }.freeze

  setup do
    def @controller.default_url_options = { locale: I18n.default_locale }
    I18n.t(:locale_name) # load the files first, or they overwrite the sentences stored below
    SENTENCES.each_value do |group, key, subject|
      I18n.backend.store_translations(:en, bot: { settings: { group => { key => { one: "[%{#{subject}}]" } } } })
    end
    @bot = create(:dca_single_asset)
    @bot.base_asset.update_columns(symbol: 'X<b>Y')
  end

  teardown { I18n.reload! }

  SENTENCES.each_key do |partial|
    test "#{partial} escapes a lone subject" do
      render partial: "bots/settings/#{partial}", locals: { bot: @bot, method: :patch, path: '/bots/1' }

      refute_includes rendered, 'X<b>Y'
      assert_includes rendered, '[X&lt;b&gt;Y'
    end
  end
end
