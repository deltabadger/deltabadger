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
end
