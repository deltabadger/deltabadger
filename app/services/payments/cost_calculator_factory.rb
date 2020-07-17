module Payments
  class CostCalculatorFactory < BaseService
    def initialize(cost_calculator: CostCalculator)
      @cost_calculator = cost_calculator
    end

    def call(eu:, subscription_plan:, discount_percent: 0, commission_percent: 0)
      @cost_calculator.new(
        base_price: subscription_plan.cost_eu,
        vat: Payments::Create::VAT_EU,
        discount_percent: discount_percent,
        commission_percent: commission_percent,
      )
    end
  end
end
