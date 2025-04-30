module Utilities
  module Time
    def self.seconds_to_midnight_utc
      now = ::Time.now.utc
      midnight = now.end_of_day
      midnight - now
    end
  end
end
