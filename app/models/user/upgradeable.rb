module User::Upgradeable
  extend ActiveSupport::Concern

  included do # rubocop:disable Metrics/BlockLength
    def available_plan_names
      case subscription.name
      when SubscriptionPlan::FREE_PLAN then available_plans_for_free_plan
      when SubscriptionPlan::BASIC_PLAN then available_plans_for_basic_plan
      when SubscriptionPlan::PRO_PLAN then available_plans_for_pro_plan
      else []
      end
    end

    private

    def available_plans_for_free_plan
      available_plans = [SubscriptionPlan::BASIC_PLAN, SubscriptionPlan::PRO_PLAN]
      available_plans << SubscriptionPlan::LEGENDARY_PLAN if legendary_plan_available?
      available_plans
    end

    def available_plans_for_basic_plan
      available_plans = [SubscriptionPlan::BASIC_PLAN, SubscriptionPlan::PRO_PLAN]
      available_plans << SubscriptionPlan::LEGENDARY_PLAN if legendary_plan_available?
      available_plans
    end

    def available_plans_for_pro_plan
      available_plans = [SubscriptionPlan::PRO_PLAN]
      available_plans << SubscriptionPlan::LEGENDARY_PLAN if legendary_plan_available?
      available_plans
    end

    def legendary_plan_available?
      @legendary_plan_available ||= SubscriptionPlan.legendary.available?
    end
  end
end
