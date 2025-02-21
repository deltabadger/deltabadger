module Utilities
  module Math
    def self.weighted_average(array, weights)
      array.zip(weights).map { |value, weight| value * weight }.sum / weights.sum.to_f
    end
  end
end
