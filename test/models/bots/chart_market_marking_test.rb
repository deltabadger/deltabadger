# frozen_string_literal: true

require 'test_helper'

# The chart values every holding at MARKET, never at the price of the fill that happened to
# come last.
#
# `metrics` can only mark at the fill — it has no prices of its own — so a transaction point
# used to re-price the WHOLE position at that one fill while the candle points around it
# priced the same holdings at market. One line, two rulers: a fill away from market showed as
# a dip at the exact moment the bot got a good fill, and every buy a sawtooth vertex.
#
# Here the candle series is the ruler. A transaction point keeps its place in time — it is a
# valuation boundary and time-weighting needs it — but its holdings are marked on the grid.
# Where the grid cannot reach a point (before the first candle, or past the last mark), the
# point keeps its fill mark, because no market price exists for it.
class ChartMarketMarkingTest < ActiveSupport::TestCase
  T0 = Time.utc(2026, 1, 1, 12, 0, 0)   # first buy, midday
  T1 = Time.utc(2026, 1, 3, 12, 0, 0)   # second buy, midday
  NOW = T0 + 14.days                    # > 300h of history -> 1.day candles

  setup do
    travel_to NOW
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
  end

  teardown { travel_back }

  # Daily candles at midnight, flat at `price` unless `prices` overrides a specific day.
  def daily_candles(from: T0 - 1.day, to: NOW, price: 100, prices: {})
    day = from.midnight
    candles = []
    while day <= to
      p = (prices[day] || price).to_d
      candles << [day, p, p, p, p, 1.to_d]
      day += 1.day
    end
    candles
  end

  def ticker_stub(base:, symbol:, candles:, id: 1)
    ticker = stub(id: id, base: base, ticker: symbol, restated_candles?: false)
    ticker.stubs(:get_candles).returns(Result::Success.new(candles))
    ticker
  end

  def exchange_stub(prices)
    exchange = stub
    exchange.stubs(:get_tickers_prices).returns(
      prices.nil? ? Result::Failure.new('down') : Result::Success.new(prices)
    )
    exchange
  end

  # Two buys: 1 base at a 90 fill, then 1 more at a 100 fill. The market is flat at 100, so
  # `metrics` marks the position at 90 then 200 — the first point is the whole position
  # re-priced at a below-market fill.
  def single_bot(candles: daily_candles, live: { 'BTCUSDT' => 100.to_d },
                 labels: [T0, T1], values: [90, 200], invested: [90, 190],
                 net_base: [1, 2], realized: [0, 0])
    bot = create(:dca_single_asset, user: create(:user))
    bot.stubs(:metrics).returns(
      { chart: { labels: labels,
                 series: [values.map(&:to_d), invested.map(&:to_d)],
                 extra_series: [net_base.map(&:to_d), realized.map(&:to_d)] },
        total_base_amount: net_base.last.to_d,
        total_realized_proceeds: realized.last.to_d,
        total_quote_amount_invested: invested.last.to_d }
    )
    bot.stubs(:ticker).returns(ticker_stub(base: 'BTC', symbol: 'BTCUSDT', candles: candles))
    bot.stubs(:exchange).returns(exchange_stub(live))
    bot
  end

  def chart(bot)
    bot.metrics_with_current_prices_and_candles(force: true)[:chart]
  end

  def value_at(chart_data, time)
    index = chart_data[:labels].index(time)
    index && chart_data[:series][0][index]
  end

  # == the reason this change exists ==

  test 'a fill below market reads as a gain, not a dip' do
    data = chart(single_bot)

    # 1 BTC held, market 100 -> worth 100 against 90 paid. `metrics` said 90/90.
    assert_equal 100.to_d, value_at(data, T0)
    assert_equal 90.to_d, data[:series][1][data[:labels].index(T0)]
  end

  test 'a fill above market reads as a loss, so it is not a one-way ratchet' do
    data = chart(single_bot(values: [110, 200], invested: [110, 210]))

    assert_equal 100.to_d, value_at(data, T0)
  end

  test 'a transaction on a flat market puts no tooth in the curve' do
    data = chart(single_bot)
    values = data[:series][0]

    # Flat market, so the only steps are the two purchases: nothing may dip.
    assert_equal values.sort, values, "value dipped: #{values.map(&:to_f).inspect}"
  end

  # == the boundary time-weighting needs ==

  test 'every transaction timestamp survives into the series' do
    data = chart(single_bot)

    assert_includes data[:labels], T0
    assert_includes data[:labels], T1
  end

  test 'a transaction is marked with the holdings it left behind, not the ones before it' do
    data = chart(single_bot)

    assert_equal 200.to_d, value_at(data, T1) # 2 base at 100, post-trade
  end

  # == coverage: bracketed by two real marks, per symbol ==

  test 'a transaction before the first candle keeps its fill mark' do
    # Candles only start the day after the first buy, so T0 has nothing below it.
    data = chart(single_bot(candles: daily_candles(from: T1 - 1.day)))

    assert_equal 90.to_d, value_at(data, T0) # untouched fill mark
    assert_equal 200.to_d, value_at(data, T1) # bracketed, market-marked
  end

  test 'the newest buy is bracketed by the live mark rather than left on its fill' do
    # A buy after the last CLOSED candle — where every actively-trading bot sits.
    recent = NOW - 2.hours
    data = chart(single_bot(labels: [T0, recent], values: [90, 150], invested: [90, 140],
                            net_base: [1, 2], realized: [0, 0]))

    assert_equal 200.to_d, value_at(data, recent) # 2 base at the live 100, not the 75 fill
  end

  test 'without a live price the tail keeps its fill mark and no endpoint is invented' do
    recent = NOW - 2.hours
    data = chart(single_bot(live: nil, labels: [T0, recent], values: [90, 150],
                            invested: [90, 140], net_base: [1, 2], realized: [0, 0]))

    assert_equal 150.to_d, value_at(data, recent)
  end

  test 'an exact hit on a candle open is used directly, with no interpolation' do
    on_the_open = T1.midnight
    data = chart(single_bot(labels: [T0, on_the_open], values: [90, 150], invested: [90, 140],
                            net_base: [1, 2], realized: [0, 0]))

    assert_equal 200.to_d, value_at(data, on_the_open)
  end

  test 'a covered point is interpolated between its two marks' do
    # 100 on the day of the buy, 200 the next day; the buy sits at noon -> 150.
    prices = { T0.midnight => 100, (T0 + 1.day).midnight => 200 }
    data = chart(single_bot(candles: daily_candles(price: 200, prices: prices)))

    assert_equal 150.to_d, value_at(data, T0) # 1 base, half-way between the marks
  end

  # == fallbacks: the mark is never invented ==

  test 'a candle fetch failure leaves the series exactly as metrics built it' do
    bot = single_bot
    bot.ticker.stubs(:get_candles).returns(Result::Failure.new('boom'))

    data = chart(bot)

    assert_equal 90.to_d, value_at(data, T0)
    assert_equal 200.to_d, value_at(data, T1)
  end

  test 'an empty candle series leaves the values as metrics built them' do
    bot = single_bot
    bot.ticker.stubs(:get_candles).returns(Result::Success.new([]))

    data = chart(bot)

    assert_equal 90.to_d, value_at(data, T0)
  end

  # == invariants the view and chart_pnl_series depend on ==

  test 'labels stay ascending and the three arrays stay the same length' do
    data = chart(single_bot)

    assert_equal data[:labels].sort, data[:labels]
    assert_equal data[:labels].size, data[:series][0].size
    assert_equal data[:labels].size, data[:series][1].size
    assert_equal data[:labels].uniq.size, data[:labels].size
  end

  test 'invested is never re-marked — it is what was paid' do
    data = chart(single_bot)

    assert_equal 90.to_d, data[:series][1][data[:labels].index(T0)]
    assert_equal 190.to_d, data[:series][1][data[:labels].index(T1)]
  end

  test 'a chart cached before this shipped still marks its live point' do
    # The pre-deploy shape: labels and series carry the live point, extra_series does not.
    bot = single_bot
    stale = bot.metrics.deep_dup
    stale[:chart][:labels] << NOW
    stale[:chart][:series][0] << 999.to_d
    stale[:chart][:series][1] << 190.to_d
    stale[:live_prices] = { 'BTC' => 100.to_d }
    bot.stubs(:metrics_with_current_prices).returns(stale)

    data = chart(bot)

    assert_equal 200.to_d, value_at(data, NOW) # the last transaction's 2 base at market, not 0
  end

  test 'a bot that sold everything is worth its cash, not its old position' do
    # The zero must survive: it is a real liquidation, not a missing extra_series row.
    data = chart(single_bot(labels: [T0, T1], values: [90, 250], invested: [90, 90],
                            net_base: [1, 0], realized: [0, 250]))

    assert_equal 250.to_d, value_at(data, T1) # realized cash only
  end

  # == index bots: coverage is a whole-portfolio property ==

  def index_bot(holdings:, candles_by_base:, live: nil)
    bot = create(:dca_index, user: create(:user))
    bot.stubs(:metrics).returns(
      { chart: { labels: [T0], series: [[100.to_d], [100.to_d]], extra_series: [holdings] },
        asset_breakdown: holdings.transform_values { |amount| { amount: amount, quote_invested: 0.to_d } },
        total_quote_amount_invested: 100.to_d }
    )
    tickers = candles_by_base.map.with_index do |(base, candles), i|
      ticker_stub(base: base, symbol: "#{base}USDT", candles: candles, id: i + 1)
    end
    bot.stubs(:tickers).returns(tickers)
    bot.stubs(:exchange).returns(exchange_stub(live))
    bot
  end

  test 'an index point is left alone when a held asset has no candles at all' do
    # AAA is priced, BBB is not — marking the point would sum a market price with a zero.
    data = chart(index_bot(holdings: { 'AAA' => 1.to_d, 'BBB' => 1.to_d },
                           candles_by_base: { 'AAA' => daily_candles, 'BBB' => [] },
                           live: { 'AAAUSDT' => 100.to_d, 'BBBUSDT' => 100.to_d }))

    assert_equal 100.to_d, value_at(data, T0) # the untouched metrics value
  end

  test 'an index point is left alone when a held asset has no live price' do
    # The live point values the missing asset at 0; it must not become an interpolation endpoint.
    recent = NOW - 2.hours
    bot = index_bot(holdings: { 'AAA' => 1.to_d, 'BBB' => 1.to_d },
                    candles_by_base: { 'AAA' => daily_candles, 'BBB' => daily_candles },
                    live: { 'AAAUSDT' => 100.to_d })
    bot.stubs(:metrics).returns(
      { chart: { labels: [recent], series: [[7.to_d], [7.to_d]], extra_series: [{ 'AAA' => 1.to_d, 'BBB' => 1.to_d }] },
        asset_breakdown: { 'AAA' => { amount: 1.to_d, quote_invested: 0.to_d },
                           'BBB' => { amount: 1.to_d, quote_invested: 0.to_d } },
        total_quote_amount_invested: 7.to_d }
    )

    data = chart(bot)

    assert_equal 7.to_d, value_at(data, recent)
  end

  # An index rotates its composition and never sells, so it goes on holding assets it no longer
  # tracks. Those have no ticker, and metrics_with_current_prices already leaves them out of the
  # live value — the chart has to agree with the headline, and demanding candle coverage for
  # them would leave nearly every point uncovered.
  test 'a held symbol with no ticker at all is excluded, matching the live value' do
    bot = index_bot(holdings: { 'AAA' => 1.to_d, 'DROPPED' => 5.to_d },
                    candles_by_base: { 'AAA' => daily_candles },
                    live: { 'AAAUSDT' => 100.to_d })
    bot.stubs(:metrics).returns(
      { chart: { labels: [T0], series: [[100.to_d], [100.to_d]],
                 extra_series: [{ 'AAA' => 1.to_d, 'DROPPED' => 5.to_d }] },
        asset_breakdown: { 'AAA' => { amount: 1.to_d, quote_invested: 0.to_d },
                           'DROPPED' => { amount: 5.to_d, quote_invested: 0.to_d } },
        total_quote_amount_invested: 100.to_d }
    )

    data = chart(bot)

    assert_equal 100.to_d, value_at(data, T0) # 1 AAA at market; DROPPED priced nowhere, counted nowhere
    assert_operator data[:labels].size, :>, 1, 'candle points must survive an untracked holding'
  end

  test 'an asset the bot no longer holds does not block coverage' do
    # Zero holdings need no price — a liquidated symbol must not freeze the whole chart.
    data = chart(index_bot(holdings: { 'AAA' => 1.to_d, 'GONE' => 0.to_d },
                           candles_by_base: { 'AAA' => daily_candles, 'GONE' => [] },
                           live: { 'AAAUSDT' => 100.to_d, 'GONEUSDT' => 100.to_d }))

    assert_equal 100.to_d, value_at(data, T0) # 1 AAA at market
  end
end
