module ExchangeTicker::TechnicallyAnalyzable
  extend ActiveSupport::Concern

  def get_rsi_value(timeframe:, period: 14)
    cache_key = "exchange_ticker_#{id}_rsi_value_#{period}_#{timeframe}"
    expires_in = Utilities::Time.seconds_to_next_candle_open(timeframe)
    rsi_value = Rails.cache.fetch(cache_key, expires_in: expires_in) do
      # Although RSI only "needs" 14 candles the calculation actually accounts for previous gains/losses.
      # A slice of 10 * period candles gives a good ratio between accuracy and performance.
      since = Time.now.utc.beginning_of_day - (10 * period * timeframe)
      result = get_candles(
        start_at: since,
        timeframe: timeframe
      )
      return result if result.failure?
      if result.data.last[0] < timeframe.ago
        return Result::Failure.new("Failed to get #{timeframe.inspect} candles since #{since} for #{ticker}")
      end

      # RubyTechnicalAnalysis doesn't play well with small token prices, so we scale them up.
      series = result.data[...-1].map { |candle| candle[4] * 10**price_decimals }
      rsi = RubyTechnicalAnalysis::RelativeStrengthIndex.new(
        series: series,
        period: period
      )
      return result if result.failure?
      unless rsi.valid?
        return Result::Failure.new("Failed to calculate #{timeframe.inspect} RSI for #{ticker} (period: #{period})")
      end

      rsi.call
    end

    Result::Success.new(rsi_value)
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
