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
    @assets['CCC'][:ticker].update!(minimum_base_size: 1) # CCC's holding below is dust
    @bot.stubs(:exited_holdings).returns([])
    @bot.stubs(:redeploy_offer).returns(0.to_d)
    @metrics = {
      total_quote_amount_invested: 200, total_amount_value_in_quote: 200, realised_pnl: 0, prices_stale: false,
      asset_values: { 'AAA' => { amount: 1, quote_invested: 100, current_value: 90, avg_price: 100, pnl_percentage: -0.1 },
                      'BBB' => { amount: 1, quote_invested: 100, current_value: 110, avg_price: 100, pnl_percentage: 0.1 },
                      'CCC' => { amount: 0.01, quote_invested: 1, current_value: 1, avg_price: 100, pnl_percentage: 0 } }
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
    assert_nil html.at_css('tr[data-symbol=CCC] .table__action a'), 'dust cannot be sold, so no button that 404s'
  end
end
