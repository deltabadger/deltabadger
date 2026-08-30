# frozen_string_literal: true

require 'test_helper'

# Closed candles never change — until a corporate action rewrites the series behind them.
# CandleSeriesCache stores them durably and only fetches the tail since the last cached candle,
# instead of refetching full history every time the old expire-at-candle-close cache rolled over.
# That makes it the one place that has to notice when the history it holds is no longer the
# venue's: append across a 10:1 split and the stored series is half in one basis, half in another.
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
    # The tail starts AT the last cached candle, so the venue's own reading of a bar already
    # stored comes back with it — plus the two new closed candles and the in-progress one.
    tail = [candle(5), candle(6), candle(7), candle(8)]
    @ticker.expects(:get_candles)
           .with { |**kw| kw[:start_at] == @since + 5.hours && kw[:timeframe] == @timeframe }
           .returns(Result::Success.new(tail))

    result = CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)

    assert_predicate result, :success?
    assert_equal 8, result.data.length                       # 0..7, no duplicate of candle(5)
    assert_equal @since + 7.hours, result.data.last[0]       # candle(8) in progress — dropped
    assert_equal result.data, result.data.uniq { |c| c[0] } # rubocop:disable Lint/AmbiguousBlockAssociation
  end

  # == a series the venue has rewritten ==

  test 'a restated overlap bar discards the series and refetches the whole window' do
    stub_full_fetch
    CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)

    travel_to Time.utc(2026, 1, 1, 8, 30)
    # The venue now reads a bar this cache already holds at a tenth of the price. A closed candle
    # does not drift; that is a 10:1 split, and everything before the seam has moved with it.
    seq = sequence('restatement')
    @ticker.expects(:get_candles).with(start_at: @since + 5.hours, timeframe: @timeframe)
           .returns(Result::Success.new([candle(5, 10.0), candle(6, 10.0), candle(7, 10.0)]))
           .in_sequence(seq)
    @ticker.expects(:get_candles).with(start_at: @since, timeframe: @timeframe)
           .returns(Result::Success.new((0..7).map { |h| candle(h, 10.0) }))
           .in_sequence(seq)

    result = CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)

    assert_predicate result, :success?
    assert_equal 8, result.data.length
    assert_equal [10.0], result.data.map { |c| c[1] }.uniq, 'a mixed-basis series survived'
  end

  test 'an overlap bar agreeing within rounding is not a restatement' do
    stub_full_fetch
    CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)

    travel_to Time.utc(2026, 1, 1, 8, 30)
    @ticker.expects(:get_candles).once
           .returns(Result::Success.new([candle(5, 100.01), candle(6), candle(7)]))

    result = CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)

    assert_equal 8, result.data.length
    assert_equal 100.0, result.data[5][1], 'the stored bar is kept, not overwritten by the reread'
  end

  test 'a rebuild stores a normalized series, whatever order the venue answers in' do
    stub_full_fetch
    CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)

    travel_to Time.utc(2026, 1, 1, 8, 30)
    seq = sequence('unordered rebuild')
    @ticker.expects(:get_candles).returns(Result::Success.new([candle(5, 10.0)])).in_sequence(seq)
    @ticker.expects(:get_candles)
           .returns(Result::Success.new([candle(2, 10.0), candle(0, 10.0), candle(1, 10.0),
                                         candle(2, 10.0)]))
           .in_sequence(seq)

    result = CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)

    # `fetch` reads the LAST row as the newest bar to decide both freshness and the next tail's
    # start, so an unsorted write sends every read after it to the wrong place.
    assert_equal [@since, @since + 1.hour, @since + 2.hours], result.data.map(&:first)
  end

  test 'a rebuild that fails leaves the stored series where it was' do
    stub_full_fetch
    CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)

    travel_to Time.utc(2026, 1, 1, 8, 30)
    seq = sequence('failed rebuild')
    @ticker.expects(:get_candles).returns(Result::Success.new([candle(5, 10.0)])).in_sequence(seq)
    @ticker.expects(:get_candles).returns(Result::Failure.new('boom')).in_sequence(seq)

    assert_predicate CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe),
                     :failure?

    travel_to Time.utc(2026, 1, 1, 6, 31)
    @ticker.expects(:get_candles).never
    assert_equal 6, CandleSeriesCache.fetch(ticker: @ticker, since: @since,
                                            timeframe: @timeframe).data.length
  end

  # == the restated series is a series of its own ==

  test 'a restated fetch reads the venue restated history and keys it apart from the raw one' do
    @ticker.stubs(:restated_candles?).returns(true)
    @ticker.expects(:get_indicator_candles).with(start_at: @since, timeframe: @timeframe)
           .returns(Result::Success.new([candle(0, 10.0), candle(1, 10.0), candle(2, 10.0)]))
    @ticker.expects(:get_candles).with(start_at: @since, timeframe: @timeframe)
           .returns(Result::Success.new([candle(0), candle(1), candle(2)]))

    restated = CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe,
                                       restated: true)
    raw = CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)

    assert_equal [10.0], restated.data.map { |c| c[1] }.uniq
    assert_equal [100.0], raw.data.map { |c| c[1] }.uniq
  end

  test 'a venue that restates nothing serves one series for both, and fetches once' do
    @ticker.stubs(:restated_candles?).returns(false)
    @ticker.expects(:get_candles).once
           .returns(Result::Success.new((0..5).map { |h| candle(h) }))
    @ticker.expects(:get_indicator_candles).never

    restated = CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe,
                                       restated: true)
    raw = CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe)

    assert_equal raw.data, restated.data
  end

  test 'a restated series missing its overlap bar rebuilds; a raw one appends' do
    @ticker.stubs(:restated_candles?).returns(true)
    @ticker.stubs(:get_indicator_candles).returns(Result::Success.new((0..5).map { |h| candle(h) }))
    CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe, restated: true)

    travel_to Time.utc(2026, 1, 1, 8, 30)
    # A bar the venue no longer returns from a start it treats as inclusive is a bar it no longer
    # has, which is the same news as one whose price moved.
    seq = sequence('missing overlap')
    @ticker.expects(:get_indicator_candles).with(start_at: @since + 5.hours, timeframe: @timeframe)
           .returns(Result::Success.new([candle(6, 10.0), candle(7, 10.0)])).in_sequence(seq)
    @ticker.expects(:get_indicator_candles).with(start_at: @since, timeframe: @timeframe)
           .returns(Result::Success.new((0..7).map { |h| candle(h, 10.0) })).in_sequence(seq)

    result = CandleSeriesCache.fetch(ticker: @ticker, since: @since, timeframe: @timeframe,
                                     restated: true)

    assert_equal [10.0], result.data.map { |c| c[1] }.uniq
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
