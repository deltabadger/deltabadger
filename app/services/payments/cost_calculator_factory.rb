module Payments
  class CostCalculatorFactory < BaseService
    def initialize(cost_calculator: CostCalculator)
      @cost_calculator = cost_calculator
    end

    # rubocop:disable Naming/UncommunicativeMethodParamName
    def call(eu:, subscription_plan:, discount_percent: 0, commission_percent: 0)
      vat = eu ? Payments::Create::VAT_EU : Payments::Create::VAT_OTHER

      @cost_calculator.new(
        base_price: subscription_plan.cost_eu,
        vat: vat,
        discount_percent: discount_percent,
        commission_percent: commission_percent
      )
    end
    # rubocop:enable Naming/UncommunicativeMethodParamName
  end
end
