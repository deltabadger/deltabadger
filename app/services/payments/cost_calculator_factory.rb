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
      vat:,
      subscription_plan:,
      current_plan:,
      days_left: 0,
      discount_percent: 0,
      commission_percent: 0
    )
      base_price = eu ? subscription_plan.cost_eu : subscription_plan.cost_other
      current_plan_base_price = eu ? current_plan.cost_eu : current_plan.cost_other

      flat_discount = if subscription_plan.name == current_plan.name
                        0
                      else
                        @flat_discount_calculator.call(
                          current_plan_base_price: current_plan_base_price,
                          current_plan_years: current_plan.years,
                          days_left: days_left
                        )
                      end

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
