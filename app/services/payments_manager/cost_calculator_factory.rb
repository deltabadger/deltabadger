module PaymentsManager
  class CostCalculatorFactory < BaseService
    EARLY_BIRD_DISCOUNT_INITIAL_VALUE = (ENV.fetch('EARLY_BIRD_DISCOUNT_INITIAL_VALUE').to_i || 0).freeze

    def initialize
      @cost_calculator = CostCalculator
    end

    def call(
      from_eu:,
      vat:,
      subscription_plan:,
      current_plan:,
      days_left: 0,
      discount_percent: 0,
      commission_percent: 0
    )
      base_price = from_eu ? subscription_plan.cost_eu : subscription_plan.cost_other
      current_plan_base_price = from_eu ? current_plan.cost_eu : current_plan.cost_other

      flat_discount = if subscription_plan.name == current_plan.name
                        0
                      else
                        begin
                          ratio = [2, days_left.to_f / (current_plan.years * 365)].min
                          (current_plan_base_price * ratio).round(2)
                        end
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

    private

    def early_bird_discount(subscription_plan)
      subscription_plan.name == 'legendary_badger' && !allowable_early_birds_count.negative? ? allowable_early_birds_count : 0
    end

    def initial_early_birds_count
      @initial_early_birds_count ||= EARLY_BIRD_DISCOUNT_INITIAL_VALUE
    end

    def purchased_early_birds_count
      @purchased_early_birds_count ||= SubscriptionsRepository.new.all_current_count('legendary_badger')
    end

    def allowable_early_birds_count
      @allowable_early_birds_count ||= initial_early_birds_count - purchased_early_birds_count
    end
  end
end
