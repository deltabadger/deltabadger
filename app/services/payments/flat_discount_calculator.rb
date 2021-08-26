module Payments
  class FlatDiscountCalculator < BaseService
    def call(current_plan_base_price:, current_plan_years:, days_left:)
      ratio = [2, days_left.to_f / (current_plan_years * 365)].min
      (current_plan_base_price * ratio).round(2)
    end
  end
end
