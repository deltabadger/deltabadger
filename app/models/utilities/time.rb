module Utilities
  module Time
    def self.seconds_to_end_of_day_utc
      now = ::Time.now.utc
      now.end_of_day - now
    end

    def self.seconds_to_end_of_five_minute_cut
      now = ::Time.now.utc
      minutes = (now.min % 5 - 5).abs - 1
      seconds = now.end_of_minute - now
      minutes * 60 + seconds
    end

    def self.seconds_to_end_of_minute
      now = ::Time.now.utc
      now.end_of_minute - now
    end

    def self.seconds_to_current_candle_close(timeframe)
      if timeframe == 1.month
        ::Time.now.utc.end_of_month - ::Time.now.utc
      elsif timeframe == 1.week
        ::Time.now.utc.end_of_week - ::Time.now.utc
      else
        timeframe_seconds = timeframe.to_i
        timestamp = ::Time.now.utc.to_i
        period_start = (timestamp / timeframe_seconds) * timeframe_seconds
        period_end = period_start + timeframe_seconds
        ::Time.at(period_end).utc - ::Time.now.utc
      end
    end
  end
end
