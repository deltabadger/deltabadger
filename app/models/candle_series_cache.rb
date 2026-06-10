# frozen_string_literal: true

# Durable, incrementally-updated store of CLOSED candles per (ticker, since, timeframe).
#
# Closed candles are immutable, so instead of letting the whole series expire at every
# candle close (which forced a full-history refetch once per candle period, per asset),
# the series is kept for 30 days and only the tail since the last cached candle is
# fetched. A cache eviction degrades gracefully to one full fetch — the old behaviour.
#
# Tail fetches are filtered by open_time > last cached open_time, so exchanges with
# inclusive or otherwise quirky start_at semantics can at worst return a few redundant
# candles, never corrupt the series. The in-progress candle is never stored.
class CandleSeriesCache
  TTL = 30.days

  def self.fetch(ticker:, since:, timeframe:)
    new(ticker: ticker, since: since, timeframe: timeframe).fetch
  end

  def initialize(ticker:, since:, timeframe:)
    @ticker = ticker
    @since = since
    @timeframe = timeframe
  end

  # Result::Success with the closed-candle series ([open_time, o, h, l, c, v] rows),
  # or the Result::Failure from the exchange.
  def fetch
    cached = Rails.cache.read(cache_key)
    return refresh if cached.blank?

    # The next closed candle opens at last_open + timeframe and closes one timeframe
    # later; before that moment there is nothing new to fetch.
    last_open = cached.last[0]
    return Result::Success.new(cached) if Time.now.utc < last_open + (2 * @timeframe)

    refresh(existing: cached)
  end

  private

  def refresh(existing: [])
    last_open = existing.last&.first
    start_at = last_open ? last_open + 1.second : @since

    result = @ticker.get_candles(start_at: start_at, timeframe: @timeframe)
    return result if result.failure?

    # A candle is closed once its period has elapsed. Checking explicitly (rather than
    # dropping the last returned candle) matters for closed markets: a weekend tail
    # fetch returns Friday's fully-closed bar last, with no in-progress bar after it.
    closed = result.data.select { |c| c[0] + @timeframe <= Time.now.utc }
    fresh = last_open ? closed.select { |c| c[0] > last_open } : closed
    # uniq+sort is cheap insurance against exchanges returning unordered/overlapping
    # tails; concat of two sorted disjoint ranges is already the common case.
    candles = (existing + fresh).uniq { |c| c[0] }.sort_by { |c| c[0] }
    # Concurrent refreshes of the same series can interleave; the losing write is a
    # valid (possibly slightly shorter) series and the next tail fetch repairs it, so
    # no read-merge-write dance is warranted for a cache.
    Rails.cache.write(cache_key, candles, expires_in: TTL) if candles.present?

    Result::Success.new(candles)
  end

  def cache_key
    "ticker_#{@ticker.id}_candle_series_v1_#{@since.to_i}_#{@timeframe.to_i}"
  end
end
