# frozen_string_literal: true

require 'test_helper'

# Pins the candle field the chart's price grid is built from, across all bot types.
#
# Exchange get_candles implementations normalize candles to
# [open_time, open, high, low, close, volume] (verified: Binance, Kraken, KuCoin).
# A grid mark is (candle[0], candle[1]) — the OPEN time and the OPEN price — so the
# pair refers to one instant. Using close (candle[4]) would put end-of-period prices
# at start-of-period timestamps, one candle out of alignment with the live mark
# appended at Time.current, and every interpolation between them would inherit the
# skew.
class ExtendedChartCandlePriceTest < ActiveSupport::TestCase
  T0 = Time.utc(2026, 1, 1, 12, 0, 0)
  NOW = T0 + 14.days # duration > 300h -> 1.day chart timeframe
  CANDLE_TIME = T0 + 1.day
  IN_PROGRESS_TIME = NOW - 12.hours # opened but not yet closed at NOW

  setup { travel_to NOW }
  teardown { travel_back }

  # open=100, close=140 — distinct so the assertions can tell them apart.
  # Two candles because CandleSeriesCache's closed-candle predicate drops the
  # trailing still-in-progress candle (open_time + timeframe > now).
  def candles(scale: 1)
    [
      [CANDLE_TIME, (100 * scale).to_d, (150 * scale).to_d, (90 * scale).to_d, (140 * scale).to_d, 1.to_d],
      [IN_PROGRESS_TIME, (140 * scale).to_d, (160 * scale).to_d, (130 * scale).to_d, (155 * scale).to_d, 1.to_d]
    ]
  end

  def ticker_stub(id:, base: 'BTC', scale: 1)
    ticker = stub(id: id, base: base, restated_candles?: false)
    ticker.stubs(:get_candles).returns(Result::Success.new(candles(scale: scale)))
    ticker
  end

  test 'single-asset chart values use the candle open price at the open-time label' do
    bot = create(:dca_single_asset, user: create(:user))
    # extra_series rows: [net_base, realized_proceeds] (locked-PnL shape). Realized 0 here, so
    # value is still purely net_base * open price.
    bot.stubs(:metrics).returns(
      { chart: { labels: [T0], series: [[200.to_d], [500.to_d]], extra_series: [[2.to_d], [0.to_d]] } }
    )
    bot.stubs(:ticker).returns(ticker_stub(id: 1))

    grid = bot.send(:chart_price_grids, bot.metrics)['BTC']

    assert_equal [CANDLE_TIME, 100.to_d], grid.first # the OPEN, not the 140 close
  end

  test "a basket's chart values use every member's candle open price" do
    bot = create(:dca_multi_asset, user: create(:user))
    bot.stubs(:metrics).returns(
      {
        chart: { labels: [T0], series: [[230.to_d], [500.to_d]],
                 extra_series: [{ 'BTC' => 2.to_d, 'ETH' => 3.to_d }] },
        asset_breakdown: { 'BTC' => {}, 'ETH' => {} }
      }
    )
    bot.stubs(:tickers).returns([ticker_stub(id: 1), ticker_stub(id: 2, base: 'ETH', scale: 10)])

    grids = bot.send(:chart_price_grids, bot.metrics)

    # Both members get their own grid, each on its OPEN (100 / 1000), not its close (140 / 1400).
    assert_equal [CANDLE_TIME, 100.to_d], grids['BTC'].first
    assert_equal [CANDLE_TIME, 1000.to_d], grids['ETH'].first
  end

  test 'index chart values use the candle open price at the open-time label' do
    bot = create(:dca_index, user: create(:user))
    bot.stubs(:metrics).returns(
      {
        chart: { labels: [T0], series: [[200.to_d], [500.to_d]], extra_series: [{ 'BTC' => 2.to_d }] },
        asset_breakdown: { 'BTC' => {} }
      }
    )
    bot.stubs(:tickers).returns([ticker_stub(id: 1)])

    grid = bot.send(:chart_price_grids, bot.metrics)['BTC']

    assert_equal [CANDLE_TIME, 100.to_d], grid.first # the OPEN, not the 140 close
  end
end
