require 'test_helper'

class Bots::CompositionMetricsViewTest < ActionView::TestCase
  include ApplicationHelper
  include BotHelper

  def setup
    @bot = create(:dca_index, user: create(:user))
    @assets = %w[AAA BBB CCC].to_h do |symbol|
      asset = create(:asset, symbol: symbol, name: "Coin #{symbol}", external_id: "coin-#{symbol.downcase}")
      ticker = create(:ticker, exchange: @bot.exchange, base_asset: asset, quote_asset: @bot.quote_asset)
      [symbol, { asset: asset, ticker: ticker }]
    end
    %w[AAA BBB CCC].each do |symbol|
      BotIndexAsset.create!(bot: @bot, asset: @assets[symbol][:asset], ticker: @assets[symbol][:ticker],
                            target_allocation: 1.0 / 3, in_index: true, entered_at: Time.current)
    end
    # CCC is a locked member the bot no longer holds: no ledger row, and a countdown to show.
    @bot.user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
    WashSaleLock.create!(user: @bot.user, asset: @assets['CCC'][:asset],
                         buy_locked_until: (Date.current + 12).beginning_of_day)
    @bot.stubs(:exited_holdings).returns([])
    @bot.stubs(:redeploy_offer).returns(0.to_d)
    @metrics = {
      total_quote_amount_invested: 200, total_amount_value_in_quote: 200, realised_pnl: 0, prices_stale: false,
      asset_values: { 'AAA' => { amount: 1, quote_invested: 100, current_value: 90, avg_price: 100, pnl_percentage: -0.1, harvestable: true },
                      'BBB' => { amount: 1, quote_invested: 100, current_value: 110, avg_price: 100, pnl_percentage: 0.1, harvestable: false } }
    }
  end

  def render_panel
    # `rendered` accumulates, so a test that renders the panel twice — to compare one state against
    # the other — would parse both at once.
    @rendered = +''
    render partial: 'bots/composition/metrics', locals: { bot: @bot, metrics: @metrics, loading: false, exited_title_key: @bot.exited_title_key }
    Nokogiri::HTML(rendered)
  end

  test 'every sellable member has a Sell, dust has none' do
    html = render_panel
    assert html.at_css('tr[data-symbol=AAA] .table__action a'), 'AAA is sellable'
    assert html.at_css('tr[data-symbol=BBB] .table__action a'), 'BBB is sellable'
    assert_nil html.at_css('tr[data-symbol=CCC] .table__action a'), 'nothing held, so no button that 404s'
  end

  test 'Sell is green only when the sale harvests a loss' do
    html = render_panel
    assert_includes html.at_css('tr[data-symbol=AAA] .table__action a')['class'], 'rbutton--success'
    assert_equal I18n.t('bot.liquidation.harvest_hint'), html.at_css('tr[data-symbol=AAA] .table__action a')['title']
    assert_not_includes html.at_css('tr[data-symbol=BBB] .table__action a')['class'], 'rbutton--success'
  end

  test 'a locked constituent is in the protection table, and nowhere else' do
    html = render_panel
    row = html.at_css('#wash_sale_list tr[data-symbol=CCC]')
    assert row, 'a locked member has a row of its own'
    assert_includes row.at_css('.table__action').text, '12'
    assert_nil html.at_css('#assets_metrics_list tr[data-symbol=CCC]'),
               'each name appears exactly once — the window is what there is to say about it'
  end

  test 'the protection table says what it is and where the rule is set' do
    html = render_panel
    assert_includes html.at_css('#wash_sale_table').text, I18n.t('bot.wash_sale.table_title')
    assert_equal settings_account_path, html.at_css('#wash_sale_table a')['href']
  end

  test 'a locked holding with a sellable remainder keeps Sell — the window blocks buying' do
    @metrics[:asset_values]['CCC'] = { amount: 1, quote_invested: 100, current_value: 90, avg_price: 100, pnl_percentage: -0.1, harvestable: true }
    cell = render_panel.at_css('#wash_sale_list tr[data-symbol=CCC] .table__action')
    assert cell.at_css('a'), 'selling is never blocked'
    assert_includes cell.text, '12'
  end

  test 'a locked quitter is in the protection table, not under Left the index' do
    bia = @bot.bot_index_assets.find_by(asset: @assets['CCC'][:asset])
    bia.update!(in_index: false, exited_at: Time.current)

    html = render_panel
    assert html.at_css('#wash_sale_list tr[data-symbol=CCC]')
    assert_nil html.at_css('#exited_metrics_list tr[data-symbol=CCC]')
  end

  test 'the protection table survives a failed price read' do
    @metrics[:asset_values] = nil

    assert render_panel.at_css('#wash_sale_list tr[data-symbol=CCC]'),
           'a constituent sold in full is exactly the one with a window and no ledger row'
  end

  test 'no locks, no table' do
    WashSaleLock.delete_all

    assert_nil render_panel.at_css('#wash_sale_table')
  end

  # --- a sale already running -------------------------------------------------------------------
  #
  # Every Sell on the bot is refused while one is working (liquidation_blocked_reason is bot-wide,
  # not per symbol), so none is offered — and the button is replaced in place rather than removed,
  # or the column resizes under a table that is fifty rows changing at once.

  def selling!
    @bot.stubs(:liquidation_selling?).returns(true)
  end

  def exit_two!
    %w[AAA BBB].each { |s| @bot.bot_index_assets.find_by(asset: @assets[s][:asset]).update!(in_index: false, exited_at: Time.current) }
    @bot.unstub(:exited_holdings)
    @bot.stubs(:exited_holdings).returns(
      %w[AAA BBB].map do |s|
        { symbol: s, ticker: @assets[s][:ticker], amount: 1, quote_invested: 100,
          current_value: 90, avg_price: 100, pnl_percentage: -0.1, harvestable: false }
      end
    )
  end

  test 'while a sale is running nothing anywhere offers Sell' do
    selling!

    assert_empty render_panel.css('a[href*="liquidation"]'),
                 'a Sell placed now would be refused, so none is offered'
  end

  test 'a row that had a Sell says a sale is running, rather than going blank' do
    selling!
    cell = render_panel.at_css('tr[data-symbol=AAA] .table__action')

    assert cell.at_css('.table__busy'), 'the placeholder stands in for the button'
    assert cell.at_css('.table__busy .rbutton'), 'the button box is kept, so the column cannot narrow'
    assert cell.at_css('.table__busy .loader--small'), 'and the spinner sits over it'
    assert_equal I18n.t('bot.liquidation.selling'), cell.at_css('.loader--small')['aria-label']
  end

  test 'the reserved box carries the translated label, which is what sets the column width' do
    selling!
    I18n.with_locale(:de) do
      ghost = render_panel.at_css('tr[data-symbol=AAA] .table__busy .rbutton')
      assert_equal I18n.t('bot.liquidation.sell', locale: :de), ghost.text.strip
      assert_equal 'true', ghost['aria-hidden'], 'a button nobody can press is not announced'
    end
  end

  test 'a row with nothing sellable gets no placeholder either' do
    # It never had a button, so there is no box to reserve.
    selling!

    assert_nil render_panel.at_css('#wash_sale_list tr[data-symbol=CCC] .table__busy')
  end

  test 'the action column survives the sale, so the table does not reflow' do
    idle = render_panel.css('#assets_metrics_list .table__action').size
    selling!

    assert_equal idle, render_panel.css('#assets_metrics_list .table__action').size
  end

  test 'Sell all is replaced by the placeholder, not merely removed' do
    exit_two!
    selling!
    header = render_panel.at_css('#exited_metrics_table .exited-header')

    assert header.at_css('.table__busy'), 'the band says the batch it started is still running'
    assert_equal I18n.t('bot.liquidation.sell_all'), header.at_css('.table__busy .rbutton').text.strip
    assert_nil header.at_css('a[href*="liquidation"]'), 'and offers no second Sell all'
  end

  test 'a single quitter gains no Sell all it never had' do
    selling!

    assert_nil render_panel.at_css('#exited_metrics_table .exited-header .table__busy')
  end

  test 'a halt outranks a sale: Clear stays reachable and nothing spins' do
    # A halt means nobody knows what happened, which is the opposite of progress — and Clear is the
    # only way out of it.
    exit_two!
    selling!
    @bot.stubs(:liquidation_halted?).returns(true)
    @bot.stubs(:liquidation_halt).returns(bases: %w[AAA], order_ids: [1], intent_id: 'abc')

    html = render_panel
    assert_includes html.at_css('#exited_metrics_table .exited-header').text, I18n.t('bot.liquidation.halted')
    assert html.at_css('#exited_metrics_table .exited-header form'), 'Clear is the one action a halt must not hide'
    assert_empty html.css('.table__busy')
  end

  test 'with nothing selling the Sell links are exactly as they were' do
    html = render_panel

    assert html.at_css('tr[data-symbol=AAA] .table__action a')
    assert_empty html.css('.table__busy')
  end
end
