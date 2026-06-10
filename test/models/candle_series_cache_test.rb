# frozen_string_literal: true

require 'test_helper'

# Closed candles never change. CandleSeriesCache stores them durably and only fetches
# the tail since the last cached candle, instead of refetching full history every time
# the old expire-at-candle-close cache rolled over.
class CandleSeriesCacheTest < ActiveSupport::TestCase
  setup do
    @store = ActiveSupport::Cache::MemoryStore.new
    Rails.stubs(:cache).returns(@store)

    @ticker = create(:ticker)
    @since = Time.utc(2026, 1, 1)
    @timeframe = 1.hour
    travel_to Time.utc(2026, 1, 1, 6, 30) # 6.5 candles after @since
  end

  teardown { travel_back }

  def candle(hours_after_since, price = 100.0)
    [@since + hours_after_since.hours, price, price, price, price, 1.0]
  end

  test 'cold cache: full fetch from since, in-progress candle dropped, series stored' do
    fetched = [candle(0), candle(1), candle(2), candle(3), candle(4), candle(5), candle(6)]
    @ticker.expects(:get_candles).with(start_at: @since, timeframe: @timeframe)
           .returns(Result::Success.new(fetched))

    result = CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)

    assert_predicate result, :success?
    assert_equal fetched[...-1], result.data # candle(6) is in progress at 06:30 — dropped
  end

  test 'warm and current: no exchange call at all' do
    CandleSeriesCache.fetch(ticker: stub_full_fetch, since: @since, timeframe: @timeframe)

    @ticker.expects(:get_candles).never
    result = CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)

    assert_predicate result, :success?
    assert_equal 6, result.data.length
  end

  test 'stale: fetches only the tail and appends, deduping any overlap' do
    stub_full_fetch
    CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)

    travel_to Time.utc(2026, 1, 1, 8, 30) # candles 6 and 7 have closed since
    # Exchange returns one overlapping already-cached candle (inclusive start_at quirk)
    # plus the two new closed candles plus the in-progress one.
    tail = [candle(5), candle(6), candle(7), candle(8)]
    @ticker.expects(:get_candles)
           .with { |**kw| kw[:start_at] > @since + 5.hours && kw[:timeframe] == @timeframe }
           .returns(Result::Success.new(tail))

    result = CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)

    assert_predicate result, :success?
    assert_equal 8, result.data.length                       # 0..7, no duplicate of candle(5)
    assert_equal @since + 7.hours, result.data.last[0]       # candle(8) in progress — dropped
    assert_equal result.data, result.data.uniq { |c| c[0] } # rubocop:disable Lint/AmbiguousBlockAssociation
  end

  test 'exchange failure: returns the failure and leaves the cache untouched' do
    stub_full_fetch
    CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)

    travel_to Time.utc(2026, 1, 1, 8, 30)
    @ticker.expects(:get_candles).returns(Result::Failure.new('boom'))

    result = CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)
    assert_predicate result, :failure?

    # cached series still intact and served once current again
    travel_to Time.utc(2026, 1, 1, 6, 31)
    @ticker.expects(:get_candles).never
    assert_equal 6, CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe).data.length
  end

  test 'different timeframe uses a different series (full fetch once after crossing)' do
    stub_full_fetch
    CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)

    @ticker.expects(:get_candles).with(start_at: @since, timeframe: 1.day)
           .returns(Result::Success.new([candle(0)]))
    CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: 1.day)
  end

  test 'empty full fetch: returns empty success, caches nothing, refetches next call' do
    @ticker.expects(:get_candles).twice.returns(Result::Success.new([]))

    result = CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)
    assert_predicate result, :success?
    assert_empty result.data

    CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe) # second exchange call expected
  end

  test 'market closed: a tail of only closed candles is kept, then served without refetch' do
    stub_full_fetch
    CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)

    travel_to Time.utc(2026, 1, 1, 8, 30)
    # Market closed after candle(6): the tail's LAST candle is fully closed (no
    # in-progress bar follows it) and must be kept, not dropped as "in progress".
    @ticker.expects(:get_candles).once.returns(Result::Success.new([candle(6)]))

    result = CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)
    assert_equal @since + 6.hours, result.data.last[0] # candle(6) kept

    # Within the freshness window of the new last candle: served from cache, no call.
    travel_to Time.utc(2026, 1, 1, 7, 59)
    @ticker.stubs(:get_candles).never
    CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)
  end

  test 'exact freshness boundary triggers a refetch' do
    stub_full_fetch
    CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)

    # last_open = 05:00; at exactly 05:00 + 2*1h = 07:00 candle(6) has just closed.
    travel_to Time.utc(2026, 1, 1, 7, 0)
    @ticker.expects(:get_candles).once.returns(Result::Success.new([candle(6)]))

    result = CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)
    assert_equal 7, result.data.length # candle(6) closed exactly now — included
  end

  private

  def stub_full_fetch
    fetched = [candle(0), candle(1), candle(2), candle(3), candle(4), candle(5), candle(6)]
    @ticker.stubs(:get_candles).returns(Result::Success.new(fetched))
    @ticker
  end
end
