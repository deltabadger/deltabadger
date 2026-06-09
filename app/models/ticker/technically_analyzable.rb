module Ticker::TechnicallyAnalyzable
  extend ActiveSupport::Concern

  # The ATH lookback is ~20 years on first computation. Exchanges that can't serve candles
  # that far back return an empty (but successful) result, which used to leave `ath` nil
  # forever and freeze "% from ATH" bots. When the deep fetch is empty we retry with these
  # progressively shorter windows so the ATH still seeds from whatever history exists. The
  # 20-year deep fetch is the primary attempt and is intentionally NOT repeated here.
  ATH_SEED_FALLBACK_WINDOWS = [5.years, 2.years, 1.year, 90.days, 30.days, 7.days].freeze

  def get_rsi_value(timeframe:, period: 14)
    cache_key = "exchange_ticker_#{id}_rsi_value_#{period}_#{timeframe}"
    expires_in = Utilities::Time.seconds_to_current_candle_close(timeframe)
    rsi_value = Rails.cache.fetch(cache_key, expires_in: expires_in) do
      # Although RSI only "needs" 15 candles the calculation actually accounts for previous gains/losses.
      # A slice of 10 * period candles gives a good ratio between accuracy and performance.
      since = Time.now.utc - ((10 * period * timeframe) + (2 * timeframe))
      result = get_candles(
        start_at: since,
        timeframe: timeframe
      )
      return result if result.failure?

      if result.data.last[0] < timeframe.ago
        return Result::Failure.new(
          "Failed to get #{timeframe.inspect} candles since #{since} for #{ticker}. " \
          'The last candle has not been closed yet.'
        )
      end

      rsi = RubyTechnicalAnalysis::RelativeStrengthIndex.new(
        series: result.data[...-1].map { |candle| candle[4] },
        period: period
      )
      return result if result.failure?
      return Result::Failure.new("Failed to calculate #{timeframe.inspect} RSI for #{ticker} (period: #{period})") unless rsi.valid?

      rsi.call.round(2)
    end

    Result::Success.new(rsi_value)
  end

  def get_sma_value(timeframe:, period: 9)
    get_moving_average_value(timeframe: timeframe, period: period, type: 'sma')
  end

  def get_ema_value(timeframe:, period: 9)
    get_moving_average_value(timeframe: timeframe, period: period, type: 'ema')
  end

  def get_wma_value(timeframe:, period: 9)
    get_moving_average_value(timeframe: timeframe, period: period, type: 'wma')
  end

  def get_moving_average_value(timeframe:, period: 9, type: 'sma')
    cache_key = "exchange_ticker_#{id}_moving_averages_values_#{period}_#{timeframe}"
    expires_in = Utilities::Time.seconds_to_current_candle_close(timeframe)
    ma_values = Rails.cache.fetch(cache_key, expires_in: expires_in) do
      # Although EMA only "needs" 21 candles the calculation actually accounts for previous gains/losses.
      # A slice of 10 * period candles gives a good ratio between accuracy and performance.
      since = Time.now.utc - ((10 * period * timeframe) + (2 * timeframe))
      result = get_candles(
        start_at: since,
        timeframe: timeframe
      )
      return result if result.failure?

      if result.data.last[0] < timeframe.ago
        return Result::Failure.new(
          "Failed to get #{timeframe.inspect} candles since #{since} for #{ticker}. " \
          'The last candle has not been closed yet.'
        )
      end

      moving_averages = RubyTechnicalAnalysis::MovingAverages.new(
        series: result.data[...-1].map { |candle| candle[4] },
        period: period
      )
      return result if result.failure?
      unless moving_averages.valid?
        return Result::Failure.new("Failed to calculate #{timeframe.inspect} Moving Averages for #{ticker} (period: #{period})")
      end

      {
        'ema' => moving_averages.ema.round(price_decimals),
        'sma' => moving_averages.sma.round(price_decimals),
        'wma' => moving_averages.wma.round(price_decimals)
      }
    end

    Result::Success.new(ma_values[type])
  end

  def get_high_of_last(duration:)
    cache_key = "exchange_ticker_#{id}_high_of_last_#{duration}"
    is_ath = duration == Float::INFINITY.seconds
    high = Rails.cache.fetch(cache_key, expires_in: 20.seconds) do
      unless is_ath
        result = high_for_duration(duration)
        return result if result.failure?

        next result.data
      end

      if ath_updated_at.present?
        # Already seeded: ath is a running maximum, so we only need the window since the
        # last update — never the full history again.
        result = high_for_duration(Time.now.utc - ath_updated_at)
        return result if result.failure?
      else
        # First computation: skip the deep walk entirely if we've already learned this
        # ticker has no candle history (guarded to the unseeded path, so a persisted ath
        # is never overwritten with nil).
        next nil if ath_seed_known_empty?

        result = high_for_duration(Time.now.utc - 20.years.ago)
        return result if result.failure?

        if result.data.blank?
          ATH_SEED_FALLBACK_WINDOWS.each do |window|
            fallback = high_for_duration(window)
            return fallback if fallback.failure?

            if fallback.data.present?
              result = fallback
              break
            end
          end
        end

        if result.data.blank?
          mark_ath_seed_empty!
          next nil
        end
      end

      # Incremental max: never downgrade a higher persisted ath (also guards the anomalous
      # ath-without-ath_updated_at state when seeding from a shorter fallback window).
      new_high = [ath, result.data].compact.max
      update!(ath: new_high, ath_updated_at: Time.now.utc) if new_high.present?
      new_high
    end

    Result::Success.new(high)
  end

  private

  def high_for_duration(duration)
    candles_timeframe = optimal_candles_timeframe_for_duration(duration)
    since = (duration + candles_timeframe).ago
    get_high(since, candles_timeframe)
  end

  def ath_seed_empty_cache_key
    "exchange_ticker_#{id}_ath_seed_empty"
  end

  def ath_seed_known_empty?
    Rails.cache.read(ath_seed_empty_cache_key).present?
  end

  def mark_ath_seed_empty!
    Rails.cache.write(ath_seed_empty_cache_key, true, expires_in: 1.hour)
  end

  def optimal_candles_timeframe_for_duration(duration)
    # These optimal timeframes are limited by Kraken's 720 candles limit
    if duration < (1 * 720).minutes
      1.minute
    elsif duration < (5 * 720).minutes
      5.minutes
    elsif duration < (15 * 720).minutes
      15.minutes
    elsif duration < (30 * 720).minutes
      30.minutes
    elsif duration < (1 * 720).hours
      1.hour
    else
      1.day
    end
  end

  def get_high(start_at, timeframe)
    result = get_candles(
      start_at: start_at,
      timeframe: timeframe
    )
    return result if result.failure?

    candles = result.data
    high = candles.empty? ? nil : candles.map { |candle| candle[2] }.max
    Result::Success.new(high)
  end
end
