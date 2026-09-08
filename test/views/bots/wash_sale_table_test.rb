require 'test_helper'

# The rule is set once for the account, so every bot has to be able to say what it is doing to that
# bot. A single-asset and a signal bot render the same panel, and both inherit Bot#locked_members —
# a per-type method on the single-asset class alone would raise on the signal bot.
class Bots::WashSaleTableViewTest < ActionView::TestCase
  include ApplicationHelper
  include BotHelper

  def setup
    @user = create(:user, wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
    @bot = create(:dca_single_asset, user: @user)
    @metrics = { total_quote_amount_invested: 100.to_d, total_amount_value_in_quote: 90.to_d,
                 total_base_amount: 1.to_d, average_buy_price: 100.to_d, pnl: -0.1 }
  end

  def lock!(bot = @bot, days: 12, source: 'bot')
    WashSaleLock.create!(user: bot.user, asset: bot.base_asset, source: source,
                         buy_locked_until: (Date.current + days).beginning_of_day)
  end

  def render_panel(bot = @bot)
    render partial: 'bots/dca_single_assets/metrics',
           locals: { bot: bot, metrics: @metrics, loading: false }
    Nokogiri::HTML(rendered)
  end

  test 'no locks, no table — an empty box says nothing worth the space' do
    assert_nil render_panel.at_css('#wash_sale_table')
  end

  test 'a locked pair is named with its countdown' do
    lock!

    row = render_panel.at_css('#wash_sale_list tr')
    assert row, 'the bot trades this pair, so its window belongs on its page'
    assert_includes row.text, 'BTC'
    assert_includes row.at_css('.table__action').text, '12'
  end

  test 'the table says what it is and where the rule is set' do
    lock!

    html = render_panel
    assert_includes html.at_css('#wash_sale_table').text, I18n.t('bot.wash_sale.table_title')
    assert_equal settings_account_path, html.at_css('#wash_sale_table a')['href']
  end

  test 'the settings link leaves the bot frame — the account page has no such frame' do
    lock!

    assert_equal '_top', render_panel.at_css('#wash_sale_table a')['data-turbo-frame']
  end

  test 'a lock armed by an exchange sale says so' do
    lock!(source: 'ledger')

    assert_includes render_panel.at_css('#wash_sale_list tr').text, I18n.t('bot.wash_sale.from_ledger')
  end

  test 'a lock on an asset this bot does not trade is not its business' do
    other = create(:asset, symbol: 'ZZZ', name: 'Coin ZZZ', external_id: 'coin-zzz')
    WashSaleLock.create!(user: @user, asset: other, buy_locked_until: 10.days.from_now)

    assert_nil render_panel.at_css('#wash_sale_table'), 'the account box is where the whole list lives'
  end

  test 'the rule switched off shows nothing, deadlines or not' do
    lock!
    @user.update!(wash_sale_enabled: false)

    assert_nil render_panel(@bot.reload).at_css('#wash_sale_table')
  end

  test 'a signal bot renders the same table' do
    bot = create(:signal_bot, user: @user, exchange: @bot.exchange,
                              base_asset: @bot.base_asset, quote_asset: @bot.quote_asset)
    lock!(bot)

    assert render_panel(bot).at_css('#wash_sale_list tr'), 'the panel is shared, so the table must be'
  end
end
