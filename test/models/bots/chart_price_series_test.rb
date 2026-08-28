# frozen_string_literal: true

require 'test_helper'

# The chart carries the PRICE of every asset it can price, alongside the value curves — the
# overlay behind "Show prices".
#
# Read off the SAME grid, at the SAME labels, with the same interpolation the values are marked
# with: a price point and a value point on one label are two readings of one number, so a curve
# and the price under it can never disagree about what the market did. Nothing is fetched for
# this — the grids are already in hand from marking the chart at market.
#
# A label the grid does not cover carries nil rather than a nearest guess: that is exactly the
# point the portfolio kept on its fill mark, and a price invented there would be the second
# ruler the whole of Bot::ChartSeries exists to remove.
class ChartPriceSeriesTest < ActiveSupport::TestCase
  T0 = Time.utc(2026, 1, 1, 12, 0, 0)   # first buy, BTC only
  T1 = T0 + 2.days                      # second buy, ETH joins
  NOW = T0 + 14.days                    # > 300h of history -> 1.day candles

  setup do
    travel_to NOW
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
  end

  teardown { travel_back }

  def daily_candles(price, from: T0 - 1.day)
    day = from.midnight
    candles = []
    while day <= NOW
      candles << [day, price.to_d, price.to_d, price.to_d, price.to_d, 1.to_d]
      day += 1.day
    end
    candles
  end

  def ticker_stub(id:, base:, price:, from: T0 - 1.day)
    ticker = stub(id: id, base: base, ticker: "#{base}USDT")
    ticker.stubs(:get_candles).returns(Result::Success.new(daily_candles(price, from: from)))
    ticker
  end

  def exchange_stub(btc: 100, eth: 50)
    exchange = stub
    exchange.stubs(:get_tickers_prices)
            .returns(Result::Success.new('BTCUSDT' => btc.to_d, 'ETHUSDT' => eth.to_d))
    exchange
  end

  def index_bot(from: T0 - 1.day, live_btc: 100)
    bot = create(:dca_index, user: create(:user))
    bot.stubs(:metrics).returns(
      { chart: { labels: [T0, T1],
                 series: [[90.to_d, 190.to_d], [90.to_d, 190.to_d]],
                 extra_series: [{ 'BTC' => 1.to_d }, { 'BTC' => 1.to_d, 'ETH' => 2.to_d }],
                 invested_series: [{ 'BTC' => 90.to_d }, { 'BTC' => 90.to_d, 'ETH' => 100.to_d }],
                 cash_series: [0.to_d, 0.to_d] },
        asset_breakdown: { 'BTC' => { amount: 1.to_d, quote_invested: 90.to_d },
                           'ETH' => { amount: 2.to_d, quote_invested: 100.to_d } },
        rebalance_cash: 0.to_d,
        total_quote_amount_invested: 190.to_d }
    )
    bot.stubs(:tickers).returns([ticker_stub(id: 1, base: 'BTC', price: 100, from: from),
                                 ticker_stub(id: 2, base: 'ETH', price: 50, from: from)])
    bot.stubs(:exchange).returns(exchange_stub(btc: live_btc))
    bot
  end

  def single_bot(live_btc: 100)
    bot = create(:dca_single_asset, user: create(:user))
    bot.stubs(:metrics).returns(
      { chart: { labels: [T0], series: [[90.to_d], [90.to_d]],
                 extra_series: [[1.to_d], [0.to_d]] },
        total_base_amount: 1.to_d, total_realized_proceeds: 0.to_d,
        total_quote_amount_invested: 90.to_d }
    )
    bot.stubs(:ticker).returns(ticker_stub(id: 1, base: 'BTC', price: 100))
    bot.stubs(:exchange).returns(exchange_stub(btc: live_btc))
    bot
  end

  def chart(bot)
    bot.metrics_with_current_prices_and_candles(force: true)[:chart]
  end

  def price_at(chart_data, symbol, time)
    chart_data[:prices][symbol][chart_data[:labels].index(time)]
  end

  # == one price series per priceable asset ==

  test 'every asset the chart can price ships a price series parallel with the labels' do
    data = chart(index_bot)

    assert_equal %w[BTC ETH], data[:prices].keys.sort
    data[:prices].each_value { |serie| assert_equal data[:labels].size, serie.size }
  end

  test 'the price is the one the holding was valued at, not a second reading' do
    data = chart(index_bot)
    i = data[:labels].index(T1)

    assert_equal data[:assets]['BTC'][:value][i], 1.to_d * price_at(data, 'BTC', T1)
    assert_equal data[:assets]['ETH'][:value][i], 2.to_d * price_at(data, 'ETH', T1)
  end

  test 'the last point carries the live price, not the last candle open' do
    data = chart(index_bot(live_btc: 120))

    assert_equal 120.to_d, data[:prices]['BTC'].last
    assert_equal 100.to_d, price_at(data, 'BTC', T1)
  end

  # == a price is never invented ==

  test 'a label the grid does not reach carries no price' do
    # Candles only start the day after the first buy, so T0 has nothing below it to interpolate
    # from — the same point that keeps its fill mark in the value series.
    data = chart(index_bot(from: T1 - 1.day))

    assert_nil price_at(data, 'BTC', T0)
    assert_equal 100.to_d, price_at(data, 'BTC', T1)
  end

  # == the bot that ships no per-asset split still ships its price ==

  test 'a single-asset bot has no :assets split but does have its price' do
    data = chart(single_bot(live_btc: 120))

    assert_nil data[:assets]
    assert_equal %w[BTC], data[:prices].keys
    assert_equal 120.to_d, data[:prices]['BTC'].last
  end
end
