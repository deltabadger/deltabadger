module Utilities
  module String
    def self.numeric?(string)
      string.to_s.match?(/\A-?\d+(\.\d+)?([eE]-?\d+)?\z/)
    end

    def self.to_boolean(string)
      return true if %w[true 1].include?(string.to_s.downcase)
      return false if %w[false 0].include?(string.to_s.downcase)

      raise ArgumentError, "Invalid boolean value: #{string}"
    end

    def self.to_duration(string)
      return nil if string.blank?

      number, unit = string.split('.')
      number.to_i.send(unit)
    end
  end
end
