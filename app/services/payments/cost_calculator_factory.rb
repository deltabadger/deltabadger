module Payments
  class CostCalculatorFactory < BaseService

    EARLY_BIRD_DISCOUNT_INITIAL_VALUE = (ENV.fetch('EARLY_BIRD_DISCOUNT_INITIAL_VALUE').to_i || 0).freeze

    def initialize(
      flat_discount_calculator: FlatDiscountCalculator.new,
      cost_calculator: CostCalculator
    )
      @flat_discount_calculator = flat_discount_calculator
      @cost_calculator = cost_calculator
    end

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
        commission_percent: commission_percent,
        early_bird_discount: early_bird_discount(subscription_plan)
      )
    end



    def early_bird_discount(subscription_plan)
      subscription_plan.name == "legendary_badger" && !allowable_early_birds_count.negative? ? allowable_early_birds_count : 0
    end

    def allowable_early_birds_count
      @allowable_early_birds_count ||= EARLY_BIRD_DISCOUNT_INITIAL_VALUE - SubscriptionsRepository.new.all_current_count('legendary_badger')
    end


  end
end
