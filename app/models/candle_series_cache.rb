# frozen_string_literal: true

# Durable, incrementally-updated store of CLOSED candles per (ticker, since, timeframe).
#
# Closed candles are immutable, so instead of letting the whole series expire at every
# candle close (which forced a full-history refetch once per candle period, per asset),
# the series is kept for 30 days and only the tail since the last cached candle is
# fetched. A cache eviction degrades gracefully to one full fetch — the old behaviour.
#
# Immutable UNTIL a corporate action, which is not a new candle but a rewrite of every old one.
# Appending a tail across a 10:1 split leaves the stored series half in one basis and half in
# another, so the tail overlaps the last stored candle by one bar and that bar is compared —
# see #restated_since?.
#
# `restated:` picks which of a venue's two histories this is: the prices as they were traded, or
# the same prices restated onto today's share basis. They are different series and are stored
# apart, on venues that have both.
class CandleSeriesCache
  TTL = 30.days

  # How far a re-read of a stored bar may move before it stops being rounding. Every real
  # restatement is a whole small ratio away — 3:2 is the narrowest in common use, and that is 33%.
  RESTATEMENT_TOLERANCE = 0.001

  def self.fetch(ticker:, since:, timeframe:, restated: false)
    new(ticker: ticker, since: since, timeframe: timeframe, restated: restated).fetch
  end

  def initialize(ticker:, since:, timeframe:, restated: false)
    @ticker = ticker
    @since = since
    @timeframe = timeframe
    # A venue that rewrites nothing has ONE history, so both callers share a key and the overlay
    # costs a cache read rather than a second request. Asked once, here, so the key and the fetch
    # can never disagree about which series this is.
    @restated = restated && ticker.restated_candles?
  end

  # Result::Success with the closed-candle series ([open_time, o, h, l, c, v] rows),
  # or the Result::Failure from the exchange.
  def fetch
    cached = Rails.cache.read(cache_key)
    return refresh if cached.blank?

    # The next closed candle opens at last_open + timeframe and closes one timeframe
    # later; before that moment there is nothing new to fetch.
    #
    # ponytail: a restatement is therefore seen no sooner than the next candle is due — up to two
    # days on a daily grid, during which the overlay carries the seam. Revalidating on every read
    # would give up what this class is for; fetch the venue's corporate-action feed if it matters.
    last_open = cached.last[0]
    return Result::Success.new(cached) if Time.now.utc < last_open + (2 * @timeframe)

    refresh(existing: cached)
  end

  private

  def refresh(existing: [])
    last_open = existing.last&.first
    # AT the last stored candle, not past it: the overlap bar is what tells a rewritten history
    # from a continuing one, and one redundant candle is what it costs.
    result = fetch_candles(start_at: last_open || @since)
    return result if result.failure?

    closed = closed_candles(result.data)
    return rebuild if last_open && restated_since?(existing.last, closed)

    fresh = last_open ? closed.select { |candle| candle[0] > last_open } : closed
    store(existing + fresh)
  end

  # The venue's own reading of a bar this cache already holds. A closed candle does not drift, so
  # a disagreement is not noise — it is the history rewritten behind us, and every bar before the
  # seam is on a basis the ones after it are not.
  #
  # A MISSING overlap says the same thing, but only where the venue restates: Alpaca documents its
  # `start` as inclusive, so a bar it will not return from that instant is a bar it no longer has.
  # Elsewhere a venue may simply read `start` exclusively, and rebuilding on that would refetch the
  # whole history on every tail forever.
  def restated_since?(last, closed)
    overlap = closed.find { |candle| candle[0] == last[0] }
    return @restated if overlap.nil?

    before = last[1].to_d
    after = overlap[1].to_d
    return false if before.zero? || after.zero?

    ((after - before) / before).abs > RESTATEMENT_TOLERANCE
  end

  # One full fetch, and never a second: a rebuild is already the expensive path, and a recursive
  # one on a venue that keeps disagreeing with itself would be unbounded. A rebuild that fails or
  # comes back empty leaves the stored series alone — a series on a stale basis still draws a
  # chart, and dropping it on the way to fixing it draws none.
  def rebuild
    result = fetch_candles(start_at: @since)
    return result if result.failure?

    candles = closed_candles(result.data)
    return Result::Success.new(candles) if candles.blank?

    store(candles)
  end

  def fetch_candles(start_at:)
    if @restated
      @ticker.get_indicator_candles(start_at: start_at, timeframe: @timeframe)
    else
      @ticker.get_candles(start_at: start_at, timeframe: @timeframe)
    end
  end

  # A candle is closed once its period has elapsed. Checking explicitly (rather than
  # dropping the last returned candle) matters for closed markets: a weekend tail
  # fetch returns Friday's fully-closed bar last, with no in-progress bar after it.
  def closed_candles(candles)
    candles.select { |candle| candle[0] + @timeframe <= Time.now.utc }
  end

  # uniq+sort is cheap insurance against exchanges returning unordered or overlapping rows, and it
  # happens HERE so that every path into the cache gets it — `fetch` reads `candles.last` as the
  # newest bar to decide both freshness and the next tail's start, so one unsorted write sends
  # every read after it to the wrong place.
  #
  # Concurrent refreshes of the same series can interleave; the losing write is a valid (possibly
  # slightly shorter) series and the next tail fetch repairs it, so no read-merge-write dance is
  # warranted for a cache.
  def store(candles)
    candles = candles.uniq { |candle| candle[0] }.sort_by { |candle| candle[0] }
    Rails.cache.write(cache_key, candles, expires_in: TTL) if candles.present?
    Result::Success.new(candles)
  end

  def cache_key
    "ticker_#{@ticker.id}_candle_series_v1#{'_restated' if @restated}_#{@since.to_i}_#{@timeframe.to_i}"
  end
end
