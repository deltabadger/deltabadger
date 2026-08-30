# frozen_string_literal: true

require 'test_helper'

# The chart's price grid must source candles through CandleSeriesCache (durable + tail
# fetch) instead of the old expire-at-candle-close inline cache.
#
# Index-bot tests stub the bot's fetch_candle_series seam with PLAIN RUBY singleton
# methods, never mocha — Task 5 makes these calls run on threads, and mocha's
# invocation bookkeeping is not thread-safe.
class ExtendedChartCandleSourceTest < ActiveSupport::TestCase
  TickerDouble = Data.define(:base)

  # Installs a plain-Ruby (thread-safe) fetch_candle_series override returning
  # per-symbol Results from a frozen hash.
  def stub_candle_series(bot, results_by_base)
    bot.define_singleton_method(:fetch_candle_series) do |ticker:, since:, timeframe:, restated: false| # rubocop:disable Lint/UnusedBlockArgument
      results_by_base.fetch(ticker.base)
    end
  end

  def index_bot_with_symbols(symbol_amounts, at:)
    bot = create(:dca_index, user: create(:user))
    bot.stubs(:metrics).returns(
      { chart: { labels: [at], series: [[10.0], [10.0]], extra_series: [symbol_amounts] },
        asset_breakdown: symbol_amounts.transform_values { {} } }
    )
    bot.stubs(:tickers).returns(symbol_amounts.keys.map { |s| TickerDouble.new(base: s) })
    bot
  end

  test 'index bot sources candles via the fetch_candle_series seam and skips failed symbols' do
    t = Time.utc(2026, 1, 1)
    bot = index_bot_with_symbols({ 'AAA' => 1.0, 'BBB' => 2.0 }, at: t)

    candles = [[t + 1.hour, 5.0, 5.0, 5.0, 5.0, 1.0]]
    stub_candle_series(bot, { 'AAA' => Result::Success.new(candles),
                              'BBB' => Result::Failure.new('boom') }.freeze)

    grids = bot.send(:chart_price_grids, bot.metrics)

    # AAA gets a grid; BBB failed and simply has none — the chart still renders, and
    # Bot::ChartSeries then leaves every point where BBB is held on its fill mark rather
    # than pricing a held asset at zero.
    assert_equal ['AAA'], grids.keys
    assert_equal [[t + 1.hour, 5.0]], grids['AAA']
  end

  test 'single-asset bot has no price grid when the candle fetch fails' do
    bot = create(:dca_single_asset, user: create(:user))
    metrics = { chart: { labels: [Time.utc(2026, 1, 1)], series: [[1.0], [1.0]], extra_series: [[1.0], [0.0]] } }
    bot.stubs(:metrics).returns(metrics)
    CandleSeriesCache.expects(:fetch).returns(Result::Failure.new('boom'))

    assert_nil bot.send(:chart_price_grids, metrics)
  end

  test 'index bot fetches candle series concurrently in bounded batches' do
    t = Time.utc(2026, 1, 1)
    symbols = ('A'..'H').map { |c| c * 3 } # 8 symbols > one batch of 6
    bot = index_bot_with_symbols(symbols.index_with { 1.0 }, at: t)

    mutex = Mutex.new
    live = 0
    peak = 0
    candles = [[t + 1.hour, 5.0, 5.0, 5.0, 5.0, 1.0]]
    bot.define_singleton_method(:fetch_candle_series) do |ticker:, since:, timeframe:, restated: false| # rubocop:disable Lint/UnusedBlockArgument
      mutex.synchronize do
        live += 1
        peak = [peak, live].max
      end
      sleep 0.02
      mutex.synchronize { live -= 1 }
      Result::Success.new(candles)
    end

    bot.send(:chart_price_grids, bot.metrics)

    assert_operator peak, :>, 1,  'fetches ran serially'
    assert_operator peak, :<=, 6, 'concurrency exceeded the bound'
  end

  test 'index bot skips symbols whose fetch raises instead of aborting the chart' do
    t = Time.utc(2026, 1, 1)
    bot = index_bot_with_symbols({ 'AAA' => 1.0, 'BAD' => 1.0 }, at: t)

    candles = [[t + 1.hour, 5.0, 5.0, 5.0, 5.0, 1.0]]
    bot.define_singleton_method(:fetch_candle_series) do |ticker:, since:, timeframe:, restated: false| # rubocop:disable Lint/UnusedBlockArgument
      raise 'unexpected explosion' if ticker.base == 'BAD'

      Result::Success.new(candles)
    end

    grids = bot.send(:chart_price_grids, bot.metrics)

    assert_equal ['AAA'], grids.keys # BAD skipped, not raised
  end

  # One ruler for every symbol and every point: a gapped symbol is INTERPOLATED across its gap
  # rather than jumped forward to its next candle. The old at-or-after alignment priced a gap at
  # a future candle, which is a different price for the same instant than the interpolation used
  # at a transaction point — two rulers again, in miniature.
  test 'a gapped symbol is interpolated across its gap and uncovered past its last candle' do
    t = Time.utc(2026, 1, 1)
    bot = index_bot_with_symbols({ 'GAP' => 1.0 }, at: t)

    # Candles at hour 1 (price 10) and hour 4 (price 40); hours 2-3 are missing.
    gap_candles = [1, 4].map { |h| [t + h.hours, (h * 10).to_d, 0, 0, 0, 0] }
    stub_candle_series(bot, { 'GAP' => Result::Success.new(gap_candles) }.freeze)

    grid = bot.send(:chart_price_grids, bot.metrics)['GAP']

    assert_equal 20.to_d, bot.send(:chart_grid_price, grid, t + 2.hours) # 1/3 of the way from 10 to 40
    assert_equal 30.to_d, bot.send(:chart_grid_price, grid, t + 3.hours)
    assert_nil bot.send(:chart_grid_price, grid, t + 5.hours) # past the last mark: uncovered, not frozen
  end
end
