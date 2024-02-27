module PaymentsManager
  class CostCalculatorFactory < BaseService
    EARLY_BIRD_DISCOUNT_INITIAL_VALUE = (ENV.fetch('EARLY_BIRD_DISCOUNT_INITIAL_VALUE').to_i || 0).freeze

    def call(
      from_eu:,
      vat:,
      subscription_plan:,
      user:
    )
      referrer = user.eligible_referrer
      @from_eu = from_eu
      @base_price = to_bigdecimal(get_base_price(subscription_plan))
      @vat = to_bigdecimal(vat)
      @flat_discount = to_bigdecimal(flat_discount_amount(subscription_plan, user))
      @discount_percent = to_bigdecimal(referrer&.discount_percent || 0)
      @commission_percent = to_bigdecimal(referrer&.commission_percent || 0)
      @early_bird_discount = to_bigdecimal(early_bird_discount(subscription_plan))
      begin
        Result::Success.new(calculate_cost_data)
      rescue StandardError => e
        Result::Failure.new(e.message)
      end
    end

    private

    def calculate_cost_data
      {
        base_price: @base_price,
        vat: @vat,
        flat_discount: @flat_discount,
        discount_percent: @discount_percent,
        commission_percent: @commission_percent,
        early_bird_discount: @early_bird_discount,
        flat_discounted_price: flat_discounted_price,
        discount_percent_amount: discount_percent_amount,
        total_vat: total_vat,
        total_price: total_price,
        commission: commission
      }
    end

    def get_base_price(plan)
      @from_eu ? plan.cost_eu : plan.cost_other
    end

    def flat_discount_amount(subscription_plan, user)
      days_left = user.plan_days_left
      current_plan = user.subscription.subscription_plan
      current_plan_base_price = get_base_price(current_plan)

      return 0 if subscription_plan.name == current_plan.name

      ratio = [2, days_left.to_f / (current_plan.years * 365)].min
      (current_plan_base_price * ratio).round(2)
    end

    def flat_discounted_price
      @flat_discounted_price ||= @base_price - @flat_discount - @early_bird_discount
    end

    def discount_percent_amount
      (flat_discounted_price * @discount_percent).round(2)
    end

    def total_vat
      total_price - discounted_price
    end

    def total_price
      @total_price ||= (discounted_price * (1 + @vat)).round(2)
    end

    def commission
      ((@base_price - @flat_discount - @early_bird_discount) * @commission_percent).round(2)
    end

    def discounted_price
      (flat_discounted_price * (1 - @discount_percent)).round(2)
    end

    # FIXME: use generic to_bigdecimal method (helper?)
    def to_bigdecimal(num, precision: 2)
      BigDecimal(format("%0.0#{precision}f", num))
    end

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
