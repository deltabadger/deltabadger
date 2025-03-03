module Utilities
  module String
    def self.numeric?(string)
      string.match?(/\A-?\d+(\.\d+)?([eE]-?\d+)?\z/)
    end

    def self.to_boolean(string)
      return true if string.to_s.downcase == 'true'
      return false if string.to_s.downcase == 'false'

      raise ArgumentError, "Invalid boolean value: #{string}"
    end
  end
end
