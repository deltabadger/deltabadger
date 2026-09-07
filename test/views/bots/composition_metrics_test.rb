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

  test 'a locked member with nothing held is listed with its countdown and no Sell' do
    row = render_panel.at_css('tr[data-symbol=CCC]')
    assert row, 'a locked member still has a row'
    assert_nil row.at_css('.table__action a')
    assert_includes row.at_css('.table__action').text, '12'
  end

  test 'a locked member with a sellable remainder shows both the countdown and Sell' do
    @metrics[:asset_values]['CCC'] = { amount: 1, quote_invested: 100, current_value: 90, avg_price: 100, pnl_percentage: -0.1, harvestable: true }
    cell = render_panel.at_css('tr[data-symbol=CCC] .table__action')
    assert cell.at_css('a'), 'still sellable'
    assert_includes cell.text, '12'
  end

  test 'a locked quitter with nothing held is listed under Left the index with its countdown' do
    bia = @bot.bot_index_assets.find_by(asset: @assets['CCC'][:asset])
    bia.update!(in_index: false, exited_at: Time.current)
    row = render_panel.at_css('#exited_metrics_list tr[data-symbol=CCC]')
    assert row, 'the quitters table is not gated on priced holdings alone'
    assert_includes row.at_css('.table__action').text, '12'
    assert_nil render_panel.at_css('#assets_metrics_list tr[data-symbol=CCC]')
  end

  test 'a panel with no priced holdings still renders the locks' do
    @metrics[:asset_values] = nil
    assert render_panel.at_css('tr[data-symbol=CCC]')
  end
end
