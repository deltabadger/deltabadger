# frozen_string_literal: true

require 'test_helper'

# The chart carries one curve per holding, alongside the portfolio's own, so pointing at a row
# of either holdings table can draw that holding alone.
#
# Marked on the SAME ruler as the portfolio line — the venue's candle grid — so the per-asset
# curves add up to the line above them and a fill away from market is not a per-asset dip
# either. A point the portfolio left on its fill mark has no per-symbol split at all: the total
# survives there, the parts do not, and the asset's curve skips it rather than reading zero.
#
# Cost basis is per asset and NOT derivable from the total: a rebalance moves basis between
# holdings, so the sold one must not keep a full basis against shrunken holdings.
class ChartAssetSeriesTest < ActiveSupport::TestCase
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

  def exchange_stub
    exchange = stub
    exchange.stubs(:get_tickers_prices)
            .returns(Result::Success.new('BTCUSDT' => 100.to_d, 'ETHUSDT' => 50.to_d))
    exchange
  end

  # 1 BTC bought at a 90 fill, then 2 ETH at a 50 fill. The market is flat at 100 / 50, so every
  # marked point values BTC at 100 and ETH at 100. Shared by the index bot and the multi-asset
  # bot — both composition types read the same asset_breakdown-shaped metrics.
  def composition_bot(factory, from: T0 - 1.day)
    bot = create(factory, user: create(:user))
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
    bot.stubs(:exchange).returns(exchange_stub)
    bot
  end

  def index_bot(from: T0 - 1.day)
    composition_bot(:dca_index, from: from)
  end

  def basket_bot
    composition_bot(:dca_multi_asset)
  end

  def chart(bot)
    bot.metrics_with_current_prices_and_candles(force: true)[:chart]
  end

  def at(chart_data, symbol, field, time)
    chart_data[:assets][symbol][field][chart_data[:labels].index(time)]
  end

  # == one curve per holding ==

  test 'every holding gets its own value and cost-basis series' do
    data = chart(index_bot)

    assert_equal %w[BTC ETH], data[:assets].keys.sort
    data[:assets].each_value do |serie|
      assert_equal data[:labels].size, serie[:value].size
      assert_equal data[:labels].size, serie[:invested].size
    end
  end

  test 'a holding is valued at market on its own, not given a share of the total' do
    data = chart(index_bot)

    assert_equal 100.to_d, at(data, 'BTC', :value, T1) # 1 BTC at 100
    assert_equal 100.to_d, at(data, 'ETH', :value, T1) # 2 ETH at 50
  end

  test 'a holding carries the basis it was bought with, not the portfolio total' do
    data = chart(index_bot)

    assert_equal 90.to_d, at(data, 'BTC', :invested, T1)
    assert_equal 100.to_d, at(data, 'ETH', :invested, T1)
  end

  test 'a holding the bot did not own yet is worth nothing, and cost nothing' do
    data = chart(index_bot)

    assert_equal 0, at(data, 'ETH', :value, T0)
    assert_equal 0, at(data, 'ETH', :invested, T0)
  end

  test 'the holdings add up to the line drawn above them' do
    data = chart(index_bot)

    data[:labels].each_index do |i|
      parts = data[:assets].values.filter_map { |serie| serie[:value][i] }
      next if parts.size < data[:assets].size # an unmarked point has no split at all

      assert_equal data[:series][0][i], parts.sum
    end
  end

  # == a point with no split is skipped, never zeroed ==

  test 'a point left on its fill mark carries no per-symbol value' do
    # Candles only start the day after the first buy, so T0 has nothing below it to interpolate
    # from and keeps the 90 `metrics` gave it.
    data = chart(index_bot(from: T1 - 1.day))

    assert_equal 90.to_d, data[:series][0][data[:labels].index(T0)]
    assert_nil at(data, 'BTC', :value, T0)
    assert_equal 100.to_d, at(data, 'BTC', :value, T1) # covered again, and marked
  end

  # == the pair ==

  test 'each member of a basket gets its own curve' do
    data = chart(basket_bot)

    assert_equal 100.to_d, at(data, 'BTC', :value, T1)
    assert_equal 100.to_d, at(data, 'ETH', :value, T1)
    assert_equal 90.to_d, at(data, 'BTC', :invested, T1)
    assert_equal 100.to_d, at(data, 'ETH', :invested, T1)
  end

  # == the one bot that needs no split ==

  test 'a single-asset bot ships no split — its one holding IS the chart' do
    bot = create(:dca_single_asset, user: create(:user))
    bot.stubs(:metrics).returns(
      { chart: { labels: [T0], series: [[90.to_d], [90.to_d]],
                 extra_series: [[1.to_d], [0.to_d]] },
        total_base_amount: 1.to_d, total_realized_proceeds: 0.to_d,
        total_quote_amount_invested: 90.to_d }
    )
    bot.stubs(:ticker).returns(ticker_stub(id: 1, base: 'BTC', price: 100))
    bot.stubs(:exchange).returns(exchange_stub)

    assert_nil chart(bot)[:assets]
  end
end
