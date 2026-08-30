require 'test_helper'

# `chart_grid_price` draws a straight line between the two candle marks around a time. The marks
# either side of a split are in different units — 2411 the day before, 254 the day after — so a
# point landing between them would be priced at something like 1300 and multiplied by a restated
# holding. That is a spike, and it is the artefact the restatement exists to remove.
class Bot::ChartSplitBoundaryTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:alpaca_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
    @usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    @klac = create(:asset, external_id: 'klac', symbol: 'KLAC')
    @bot = create(:dca_single_asset, user: @user, exchange: @exchange, with_api_key: false,
                                     base_asset: @klac, quote_asset: @usd)
    @split_at = 5.days.ago.change(usec: 0)
    # The bot has to have traded the symbol on this venue for a restatement to be its business.
    @buy = create(:transaction, bot: @bot, exchange: @exchange, base: 'KLAC', quote: 'USD',
                                side: :buy, amount: 2, amount_exec: 2, price: 1000,
                                quote_amount: 2000, quote_amount_exec: 2000,
                                created_at: @split_at - 8.days)
  end

  # A daily grid across the split: 1000 a day before, 100 a day after — one restatement, no drift.
  def daily_grid
    marks = (-8..-6).map { |d| [@split_at + d.days, 1000.to_d] } +
            (0..4).map { |d| [@split_at + d.days, 100.to_d] }
    { 'KLAC' => marks }
  end

  def split!(ratio: '10:1')
    create(:account_transaction, user: @user, api_key: @api_key, exchange: @exchange,
                                 entry_type: :adjustment, base_currency: 'KLAC', base_amount: 90,
                                 quote_currency: nil, quote_amount: nil, transacted_at: @split_at,
                                 raw_data: { 'corporate_action' => 'split', 'split_ratio' => ratio })
  end

  test 'the grid gains a mark on each side of the boundary, in its own units' do
    split!

    pinned = @bot.chart_split_pinned_grids(daily_grid)['KLAC']

    assert_equal 1000.to_d, pinned.find { |at, _price| at == @split_at - 1.second }&.last
    assert_equal 100.to_d, pinned.find { |at, _price| at == @split_at }&.last
  end

  test 'a price read between the two bases lands in one of them, not between' do
    split!
    # Half a day before the split: strictly between the last pre-split mark and the first
    # post-split one, which is exactly where a straight line between them is meaningless.
    midway = @split_at - 12.hours

    unpinned = @bot.send(:chart_grid_price, daily_grid['KLAC'], midway)
    pinned = @bot.send(:chart_grid_price, @bot.chart_split_pinned_grids(daily_grid)['KLAC'], midway)

    assert_operator unpinned, :<, 1000.to_d
    assert_operator unpinned, :>, 100.to_d, 'without the pins it reads a price nobody quoted'
    assert_equal 1000.to_d, pinned, 'still on the pre-split basis, where the holding also is'
  end

  test 'a price read at the boundary itself is the post-split one' do
    split!

    pinned = @bot.chart_split_pinned_grids(daily_grid)['KLAC']

    assert_equal 100.to_d, @bot.send(:chart_grid_price, pinned, @split_at)
    assert_equal 1000.to_d, @bot.send(:chart_grid_price, pinned, @split_at - 1.second)
  end

  test 'a grid that does not reach both sides of the split is left alone' do
    split!
    one_sided = { 'KLAC' => (0..4).map { |d| [@split_at + d.days, 100.to_d] } }

    assert_equal one_sided, @bot.chart_split_pinned_grids(one_sided)
  end

  test 'a bot with no restatement gets its grid back untouched' do
    assert_equal daily_grid, @bot.chart_split_pinned_grids(daily_grid)
  end

  test 'the marked value series has neither spike nor step across the split' do
    split!
    grid = daily_grid
    @bot.define_singleton_method(:chart_price_grids) { |_metrics_data| grid }
    Exchanges::Alpaca.any_instance.stubs(:get_last_price).returns(Result::Success.new(100.to_d))
    Exchanges::Alpaca.any_instance.stubs(:get_tickers_prices)
                     .returns(Result::Success.new('KLACUSD' => 100.to_d))

    series = @bot.metrics_with_current_prices_and_candles(force: true)[:chart][:series][0]

    # Two shares at 1000, then twenty at 100: 2000 throughout, at every point on the grid.
    assert_operator series.size, :>=, 4
    series.each { |value| assert_in_delta 2000.to_d, value.to_d, 1.to_d, "series: #{series.inspect}" }
  end
end
