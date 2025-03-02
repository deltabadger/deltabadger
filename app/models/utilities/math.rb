module Utilities
  module Math
    def self.weighted_average(array, weights)
      array.zip(weights).map { |value, weight| value * weight }.sum / weights.sum.to_f
    end

    def self.percentage_change(previous, current)
      return nil if previous.zero?

      ((current - previous) / previous) * 100
    end
  end
end
