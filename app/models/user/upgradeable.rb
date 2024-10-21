module User::Upgradeable
  extend ActiveSupport::Concern

  included do # rubocop:disable Metrics/BlockLength
    def available_plans
      case subscription_name
      when 'free' then available_plans_for_free_plan
      when 'standard' then available_plans_for_standard_plan
      when 'pro' then available_plans_for_pro_plan
      else []
      end
    end

    private

    def available_plans_for_free_plan
      available_plans = %w[standard pro]
      available_plans << 'legendary' if SubscriptionPlan.legendary_plan_available?
      available_plans
    end

    def available_plans_for_standard_plan
      available_plans = %w[pro]
      available_plans << 'standard' if standard_plan_eligibility?
      available_plans << 'legendary' if SubscriptionPlan.legendary_plan_available?
      available_plans
    end

    def available_plans_for_pro_plan
      available_plans = []
      available_plans << 'legendary' if SubscriptionPlan.legendary_plan_available?
      available_plans
    end

    def standard_plan_eligibility?
      subscription.end_time > Time.current + 1.years
    end
  end
end
