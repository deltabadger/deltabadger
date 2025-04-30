module Utilities
  module Number
    def self.to_bigdecimal(num, precision: 2)
      BigDecimal(format("%0.0#{precision}f", num))
    end

    def self.decimals(num)
      # Convert to string and remove trailing zeros after decimal
      str = num.to_s.sub(/\.?0+$/, '')

      # If no decimal point, return 0
      return 0 unless str.include?('.')

      # Count digits after decimal point
      str.split('.').last.length
    end
  end
end
