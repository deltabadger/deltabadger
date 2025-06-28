module Ticker::TechnicallyAnalyzable
  extend ActiveSupport::Concern

  def get_rsi_value(timeframe:, period: 14)
    cache_key = "exchange_ticker_#{id}_rsi_value_#{period}_#{timeframe}"
    expires_in = Utilities::Time.seconds_to_current_candle_close(timeframe)
    rsi_value = Rails.cache.fetch(cache_key, expires_in: expires_in) do
      # Although RSI only "needs" 15 candles the calculation actually accounts for previous gains/losses.
      # A slice of 10 * period candles gives a good ratio between accuracy and performance.
      since = Time.now.utc - (10 * period * timeframe + 2 * timeframe)
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
      unless rsi.valid?
        return Result::Failure.new("Failed to calculate #{timeframe.inspect} RSI for #{ticker} (period: #{period})")
      end

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
      since = Time.now.utc - (10 * period * timeframe + 2 * timeframe)
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
      duration = Time.now.utc - (ath_updated_at || 20.years.ago) if is_ath

      candles_timeframe = optimal_candles_timeframe_for_duration(duration)
      since = (duration + candles_timeframe).ago
      result = get_high(since, candles_timeframe)
      return result if result.failure?

      if is_ath
        high = [ath, result.data].compact.max
        update!(ath: high, ath_updated_at: Time.now.utc) if high.present?
        high
      else
        result.data
      end
    end

    Result::Success.new(high)
  end

  private

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
