require 'test_helper'

class Ticker::TechnicallyAnalyzableTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:kucoin_exchange)
    @ticker = create(:ticker, exchange: @exchange, base_symbol: 'AVA', quote_symbol: 'USDT')
  end

  # A normalized candle is [time, open, high, low, close, volume]; get_high reads index 2.
  def normalized_candle(high:, at: Time.now.utc - 1.day)
    [at, 100.to_d, high.to_d, 90.to_d, 110.to_d, 50.to_d]
  end

  # ---- ATH seeding resilience ----
  #
  # The ATH lookback is ~20 years on first computation. On exchanges that cannot serve
  # candles that far back the deep fetch returns an empty (but successful) result, which
  # previously left `ath` nil forever and froze "% from ATH" bots. The seed must fall
  # back to progressively shorter windows until it finds data.

  test 'seeds ATH from a shorter window when the deep fetch comes back empty' do
    cutoff = 100.days.ago
    candle = normalized_candle(high: 120)
    @ticker.define_singleton_method(:get_candles) do |**kw|
      Result::Success.new(kw[:start_at] >= cutoff ? [candle] : [])
    end

    result = @ticker.get_high_of_last(duration: Float::INFINITY.seconds)

    assert_predicate result, :success?
    assert_equal 120.to_d, result.data
    @ticker.reload
    assert_equal 120.to_d, @ticker.ath
    assert_not_nil @ticker.ath_updated_at
  end

  test 'propagates a failure from the deep fetch without trying shorter windows' do
    calls = 0
    @ticker.define_singleton_method(:get_candles) do |**_kw|
      calls += 1
      Result::Failure.new('boom')
    end

    result = @ticker.get_high_of_last(duration: Float::INFINITY.seconds)

    assert_predicate result, :failure?
    assert_equal 1, calls, 'a real failure must not be masked by walking shorter windows'
    @ticker.reload
    assert_nil @ticker.ath
  end

  test 'propagates a failure that occurs during a fallback window and does not seed' do
    # The deep + long windows are empty; a shorter fallback window errors. That failure must
    # propagate (not be swallowed by continuing the walk).
    @ticker.define_singleton_method(:get_candles) do |**kw|
      if kw[:start_at] >= 200.days.ago
        Result::Failure.new('boom')
      else
        Result::Success.new([])
      end
    end

    result = @ticker.get_high_of_last(duration: Float::INFINITY.seconds)

    assert_predicate result, :failure?
    @ticker.reload
    assert_nil @ticker.ath
  end

  test 'a stale persisted ATH is never downgraded by a shorter-window seed' do
    # Anomalous state: ath present but ath_updated_at nil (manual SQL / migration artifact).
    @ticker.update_columns(ath: 200, ath_updated_at: nil)
    cutoff = 100.days.ago
    candle = normalized_candle(high: 150)
    @ticker.define_singleton_method(:get_candles) do |**kw|
      Result::Success.new(kw[:start_at] >= cutoff ? [candle] : [])
    end

    result = @ticker.get_high_of_last(duration: Float::INFINITY.seconds)

    assert_predicate result, :success?
    assert_equal 200.to_d, result.data, 'must keep the higher persisted ath, not downgrade to 150'
    @ticker.reload
    assert_equal 200.to_d, @ticker.ath
  end

  test 'already-seeded ATH uses a short lookback and keeps the incremental max' do
    @ticker.update!(ath: 200, ath_updated_at: 2.days.ago)
    seen = []
    candle = normalized_candle(high: 150, at: Time.now.utc - 1.hour)
    @ticker.define_singleton_method(:get_candles) do |**kw|
      seen << kw[:start_at]
      Result::Success.new([candle])
    end

    result = @ticker.get_high_of_last(duration: Float::INFINITY.seconds)

    assert_predicate result, :success?
    assert_equal 200.to_d, result.data, 'incremental max must keep the higher stored ATH'
    assert_equal 1, seen.size, 'a seeded ATH must not trigger the fallback walk'
    assert_operator seen.first, :>=, 3.days.ago, 'must use a short lookback, not ~20 years'
  end

  test 'suppresses the seed walk on subsequent ticks when every window is empty' do
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    calls = 0
    @ticker.define_singleton_method(:get_candles) do |**_kw|
      calls += 1
      Result::Success.new([])
    end

    freeze_time do
      first = @ticker.get_high_of_last(duration: Float::INFINITY.seconds)
      assert_predicate first, :success?
      assert_nil first.data
      walked = calls
      assert_operator walked, :>=, 2, 'first tick should attempt the deep fetch plus fallback windows'

      travel 30.seconds # past the 20s result cache, still within the negative memo
      second = @ticker.get_high_of_last(duration: Float::INFINITY.seconds)
      assert_nil second.data
      assert_equal walked, calls, 'the negative memo should suppress re-walking empty windows'
    end

    @ticker.reload
    assert_nil @ticker.ath
  end
end
