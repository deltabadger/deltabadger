module Payments
  class CostCalculatorFactory < BaseService
    def initialize(
      flat_discount_calculator: FlatDiscountCalculator.new,
      cost_calculator: CostCalculator
    )
      @flat_discount_calculator = flat_discount_calculator
      @cost_calculator = cost_calculator
    end

    # rubocop:disable Naming/UncommunicativeMethodParamName, Metrics/ParameterLists
    def call(
      eu:,
      subscription_plan:,
      current_plan:,
      days_left: 0,
      discount_percent: 0,
      commission_percent: 0
    )
      base_price = eu ? subscription_plan.cost_eu : subscription_plan.cost_other
      vat = eu ? Payments::Create::VAT_EU : Payments::Create::VAT_OTHER
      current_plan_base_price = eu ? current_plan.cost_eu : current_plan.cost_other

      flat_discount = @flat_discount_calculator.call(
        current_plan_base_price: current_plan_base_price,
        current_plan_years: current_plan.years,
        days_left: days_left
      )

      @cost_calculator.new(
        base_price: base_price,
        vat: vat,
        flat_discount: flat_discount,
        discount_percent: discount_percent,
        commission_percent: commission_percent
      )
    end
    # rubocop:enable Naming/UncommunicativeMethodParamName, Metrics/ParameterLists
  end
end
