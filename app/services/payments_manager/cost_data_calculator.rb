module PaymentsManager
  class CostDataCalculator < BaseService
    def call(
      user:,
      country: nil,
      payment: nil,
      subscription_plan: nil,
      referrer: user.eligible_referrer,
      legendary_badger_discount: nil
    )
      validation_result = validate_params(country, subscription_plan, payment)
      return validation_result if validation_result.failure?

      @legendary_badger_discount = legendary_badger_discount
      @base_price = to_bigdecimal(get_base_price(@subscription_plan))
      @flat_discount = to_bigdecimal(flat_discount_amount(user))
      @discount_percent = to_bigdecimal(referrer&.discount_percent || 0)
      @commission_percent = to_bigdecimal(referrer&.commission_percent || 0)
      begin
        Result::Success.new(calculate_cost_data)
      rescue StandardError => e
        Result::Failure.new(e.message)
      end
    end

    private

    def validate_params(country, subscription_plan, payment)
      if country.present? && subscription_plan.present?
        @from_eu = country.eu?
        @vat = to_bigdecimal(country.vat)
        @subscription_plan = subscription_plan
      elsif payment.present?
        @from_eu = payment.eu?
        @vat = to_bigdecimal(VatRate.find_by!(country: payment.country).vat)
        @subscription_plan = payment.subscription_plan
      else
        Result::Failure.new('Either user & country & subscription_plan or user & payment must be provided')
      end
      Result::Success.new
    end

    def calculate_cost_data
      {
        base_price: @base_price,
        vat: @vat,
        flat_discount: @flat_discount,
        discount_percent: @discount_percent,
        commission_percent: @commission_percent,
        flat_discounted_price: flat_discounted_price,
        discount_percent_amount: discount_percent_amount,
        total_vat: total_vat,
        total_price: total_price,
        commission: commission,
        subscription_plan_name: @subscription_plan.name
      }
    end

    def get_base_price(plan)
      @from_eu ? plan.cost_eu : plan.cost_other
    end

    def flat_discount_amount(user)
      current_plan = user.subscription.subscription_plan
      return 0 if @subscription_plan.name == current_plan.name

      plan_years_left = user.plan_days_left.to_f / 365
      discount_multiplier = [2, plan_years_left / current_plan.years].min
      (get_base_price(current_plan) * discount_multiplier).round(2)
    end

    def flat_discounted_price
      # HACK: force a price of at least 1 so a payment can be done to upgrade, even if the price should be 0
      # TODO: allow prices of 0 and let the controller upgrade the plan without payment in this case
      @flat_discounted_price ||= [1, @base_price - @flat_discount - legendary_badger_discount].max
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
      (flat_discounted_price * @commission_percent).round(2)
    end

    def discounted_price
      (flat_discounted_price * (1 - @discount_percent)).round(2)
    end

    def legendary_badger_discount
      return 0 unless @subscription_plan.name == 'legendary_badger'

      @legendary_badger_discount ||= legendary_badger_stats[:legendary_badger_discount]
    end

    def legendary_badger_stats
      @legendary_badger_stats ||= PaymentsManager::LegendaryBadgerStatsCalculator.call.data
    end

    # FIXME: use generic to_bigdecimal method (helper?)
    def to_bigdecimal(num, precision: 2)
      BigDecimal(format("%0.0#{precision}f", num))
    end
  end
end
