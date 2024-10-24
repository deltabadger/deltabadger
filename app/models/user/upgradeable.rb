module User::Upgradeable
  extend ActiveSupport::Concern

  included do # rubocop:disable Metrics/BlockLength
    def available_plans
      case subscription_name
      when SubscriptionPlan::FREE_PLAN then available_plans_for_free_plan
      when SubscriptionPlan::STANDARD_PLAN then available_plans_for_standard_plan
      when SubscriptionPlan::PRO_PLAN then available_plans_for_pro_plan
      else []
      end
    end

    private

    def available_plans_for_free_plan
      available_plans = [SubscriptionPlan::STANDARD_PLAN, SubscriptionPlan::PRO_PLAN]
      available_plans << SubscriptionPlan::LEGENDARY_PLAN if legendary_plan_available?
      available_plans
    end

    def available_plans_for_standard_plan
      available_plans = [SubscriptionPlan::PRO_PLAN]
      available_plans << SubscriptionPlan::STANDARD_PLAN if standard_plan_eligibility?
      available_plans << SubscriptionPlan::LEGENDARY_PLAN if legendary_plan_available?
      available_plans
    end

    def available_plans_for_pro_plan
      available_plans = []
      available_plans << SubscriptionPlan::LEGENDARY_PLAN if legendary_plan_available?
      available_plans
    end

    def standard_plan_eligibility?
      subscription.end_time > Time.current + 1.years
    end

    def legendary_plan_available?
      @legendary_plan_available ||= SubscriptionPlan.legendary.available?
    end
  end
end
