# frozen_string_literal: true

require 'test_helper'

# Pins the candle field used for extended-chart interpolation across all bot types.
#
# Exchange get_candles implementations normalize candles to
# [open_time, open, high, low, close, volume] (verified: Binance, Kraken, KuCoin).
# The extended chart labels each point with candle[0] — the candle's OPEN time —
# so the value must use candle[1], the OPEN price, to keep the (time, price) pair
# consistent. Using close (candle[4]) would plot end-of-period prices at
# start-of-period timestamps, one candle out of alignment with the live
# current-price point appended at Time.current.
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
    ticker = stub(id: id, base: base)
    ticker.stubs(:get_candles).returns(Result::Success.new(candles(scale: scale)))
    ticker
  end

  test 'single-asset chart values use the candle open price at the open-time label' do
    bot = create(:dca_single_asset, user: create(:user))
    bot.stubs(:metrics).returns(
      { chart: { labels: [T0], series: [[200.to_d], [500.to_d]], extra_series: [[2.to_d]] } }
    )
    bot.stubs(:ticker).returns(ticker_stub(id: 1))

    data = bot.send(:get_extended_chart_data_with_candles_data).data

    assert_equal [CANDLE_TIME], data[:labels]
    assert_equal [2.to_d * 100], data[:series][0] # base_amount * OPEN, not * 140 (close)
    assert_equal [500.to_d], data[:series][1]
  end

  test 'dual-asset chart values use both candle open prices at the open-time label' do
    bot = create(:dca_dual_asset, user: create(:user))
    bot.stubs(:metrics).returns(
      { chart: { labels: [T0], series: [[230.to_d], [500.to_d]], extra_series: [[2.to_d], [3.to_d]] } }
    )
    bot.stubs(:ticker0).returns(ticker_stub(id: 1))
    bot.stubs(:ticker1).returns(ticker_stub(id: 2, scale: 10))

    data = bot.send(:get_extended_chart_data_with_candles_data).data

    assert_equal [CANDLE_TIME], data[:labels]
    # 2 * open0 (100) + 3 * open1 (1000), not the closes (140 / 1400)
    assert_equal [(2.to_d * 100) + (3.to_d * 1000)], data[:series][0]
    assert_equal [500.to_d], data[:series][1]
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

    data = bot.send(:get_extended_chart_data_with_candles_data).data

    assert_equal [CANDLE_TIME], data[:labels]
    assert_equal [2.to_d * 100], data[:series][0] # amount * OPEN, not * 140 (close)
    assert_equal [500.to_d], data[:series][1]
  end
end
