# frozen_string_literal: true

require 'test_helper'

# The extended chart must source candles through CandleSeriesCache (durable + tail
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
    bot.define_singleton_method(:fetch_candle_series) do |ticker:, since:, timeframe:| # rubocop:disable Lint/UnusedBlockArgument
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

    result = bot.send(:get_extended_chart_data_with_candles_data)

    assert_predicate result, :success?
    # AAA contributes 1.0 * 5.0; BBB skipped (failed) — chart still renders
    assert_equal [5.0], result.data[:series][0]
  end

  test 'single-asset bot aborts the extended chart when the candle fetch fails' do
    bot = create(:dca_single_asset, user: create(:user))
    bot.stubs(:metrics).returns({ chart: { labels: [Time.utc(2026, 1, 1)],
                                           series: [[1.0], [1.0]], extra_series: [[1.0]] } })
    CandleSeriesCache.expects(:fetch).returns(Result::Failure.new('boom'))

    assert_predicate bot.send(:get_extended_chart_data_with_candles_data), :failure?
  end

  test 'index bot fetches candle series concurrently in bounded batches' do
    t = Time.utc(2026, 1, 1)
    symbols = ('A'..'H').map { |c| c * 3 } # 8 symbols > one batch of 6
    bot = index_bot_with_symbols(symbols.index_with { 1.0 }, at: t)

    mutex = Mutex.new
    live = 0
    peak = 0
    candles = [[t + 1.hour, 5.0, 5.0, 5.0, 5.0, 1.0]]
    bot.define_singleton_method(:fetch_candle_series) do |ticker:, since:, timeframe:| # rubocop:disable Lint/UnusedBlockArgument
      mutex.synchronize do
        live += 1
        peak = [peak, live].max
      end
      sleep 0.02
      mutex.synchronize { live -= 1 }
      Result::Success.new(candles)
    end

    result = bot.send(:get_extended_chart_data_with_candles_data)

    assert_predicate result, :success?
    assert_operator peak, :>, 1,  'fetches ran serially'
    assert_operator peak, :<=, 6, 'concurrency exceeded the bound'
  end

  test 'index bot skips symbols whose fetch raises instead of aborting the chart' do
    t = Time.utc(2026, 1, 1)
    bot = index_bot_with_symbols({ 'AAA' => 1.0, 'BAD' => 1.0 }, at: t)

    candles = [[t + 1.hour, 5.0, 5.0, 5.0, 5.0, 1.0]]
    bot.define_singleton_method(:fetch_candle_series) do |ticker:, since:, timeframe:| # rubocop:disable Lint/UnusedBlockArgument
      raise 'unexpected explosion' if ticker.base == 'BAD'

      Result::Success.new(candles)
    end

    result = bot.send(:get_extended_chart_data_with_candles_data)

    assert_predicate result, :success?
    assert_equal [5.0], result.data[:series][0] # AAA only; BAD skipped, not raised
  end
end
