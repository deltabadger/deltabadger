module User::Upgradeable
  extend ActiveSupport::Concern

  def available_plan_names
    return [] unless subscription&.name.present?

    base_plans = {
      SubscriptionPlan::FREE_PLAN => [SubscriptionPlan::BASIC_PLAN, SubscriptionPlan::PRO_PLAN],
      SubscriptionPlan::BASIC_PLAN => [SubscriptionPlan::BASIC_PLAN, SubscriptionPlan::PRO_PLAN],
      SubscriptionPlan::PRO_PLAN => [SubscriptionPlan::PRO_PLAN]
    }

    available_plans = base_plans[subscription.name] || []
    available_plans << SubscriptionPlan::LEGENDARY_PLAN if legendary_plan_available?
    available_plans
  end

  private

  def legendary_plan_available?
    @legendary_plan_available ||= SubscriptionPlan.legendary.available?
  end
end
