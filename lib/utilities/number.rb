module Utilities
  module Number
    def self.to_bigdecimal(num, precision: 2)
      BigDecimal(format("%0.0#{precision}f", num))
    end
  end
end
