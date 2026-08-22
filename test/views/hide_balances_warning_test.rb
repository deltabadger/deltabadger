# frozen_string_literal: true

require 'test_helper'

# The below-minimums warning is broadcast after an execution rather than opened by a click, so it
# arrives on screen unbidden. Of its three variants only the dual-asset "one leg missed" wording
# states the user's own money — it opens by saying what the bot spent.
class HideBalancesWarningTest < ActionView::TestCase
  def render_warning(bot, hidden:)
    bot.user.update!(hide_balances: hidden)
    render partial: 'bots/dca_dual_assets/warning_below_minimums', locals: {
      bot: bot, missed_count: 1,
      bought_quote_amount: 4321, quote_symbol: 'USD', bought_symbol: 'BTC',
      missed_symbol: 'ETH', missed_minimum_base_size: 0.01, missed_minimum_quote_size: 10,
      exchange_name: 'Binance'
    }
  end

  setup do
    @bot = create(:dca_dual_asset)
  end

  test 'the warning states what was spent when balances are shown' do
    render_warning(@bot, hidden: false)

    assert_match '4321', rendered
  end

  # The single-asset wording IS that sentence with the money clause gone, and it is already
  # translated in all fifteen locales — which is why it stands in rather than a new key.
  test 'the warning keeps the missed leg and its minimum but not the spend' do
    render_warning(@bot, hidden: true)

    assert_no_match(/4321/, rendered)
    assert_match 'ETH', rendered
    assert_match 'Binance', rendered
    assert_match I18n.t('bot.warning.buffer_explanation', count: 1), rendered
  end
end
