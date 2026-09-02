# frozen_string_literal: true

require 'test_helper'

# The Smart Intervals split is the one conversational input carrying a `min`, so it is the one that
# gets rejected in a sentence. The browser's own answer to that — "Value must be greater than or
# equal to 5." — names a number without naming the rule behind it, so the field carries its own.
class SmartIntervalsWidgetTest < ActionView::TestCase
  setup do
    def @controller.default_url_options = { locale: I18n.default_locale }
  end

  def render_widget(bot)
    render partial: 'bots/settings/smart_intervals',
           locals: { bot: bot, method: :patch, path: '/bots/1' }
    rendered
  end

  test 'the buy split carries the explanation and the floor it belongs to' do
    bot = create(:dca_multi_asset)
    bot.settings = bot.settings.merge('quote_amount' => 100.0, 'interval' => 'week')
    bot.smart_intervaled = true

    html = render_widget(bot)
    field = Nokogiri::HTML.fragment(html).at('input[name*="smart_interval_quote_amount"]')

    assert_equal bot.smart_interval_minimum_message(:quote), field['data-html5-range-underflow-message']
    assert_equal bot.smart_interval_minimum(:quote)[:value].to_s, field['min']
    assert_includes field['data-html5-range-underflow-message'], bot.exchange.name
  end

  test 'the sell split carries its own explanation' do
    bot = create(:dca_single_asset)
    bot.settings = bot.settings.merge('direction' => 'selling', 'sell_denomination' => 'base',
                                      'sell_amount' => 1.0, 'sell_interval' => 'day')
    bot.smart_intervaled = true
    assert_predicate bot, :sells_base_amount?

    html = render_widget(bot)
    field = Nokogiri::HTML.fragment(html).at('input[name*="smart_interval_base_amount"]')

    assert_equal bot.smart_interval_minimum_message(:base), field['data-html5-range-underflow-message']
    assert_equal bot.smart_interval_minimum(:base)[:value].to_s, field['min']
  end
end
