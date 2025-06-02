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

    def self.seconds_to_next_candle_open(timeframe)
      raise "Missing implementation for #{timeframe.inspect}" unless timeframe <= 1.day

      timeframe.seconds - (::Time.now.utc.to_f % timeframe.to_f)
    end
  end
end
