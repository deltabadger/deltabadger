module User::Upgradeable
  extend ActiveSupport::Concern

  def available_plan_names
    return [] if subscription.legendary?

    available_plans = base_plans[subscription.name] || []
    available_plans << SubscriptionPlan::LEGENDARY_PLAN if legendary_plan_available?
    available_plans
  end

  private

  def legendary_plan_available?
    @legendary_plan_available ||= SubscriptionPlan.legendary.available?
  end

  def base_plans
    {
      SubscriptionPlan::FREE_PLAN => [
        SubscriptionPlan::MINI_PLAN,
        SubscriptionPlan::STANDARD_PLAN,
        SubscriptionPlan::PRO_PLAN,
        SubscriptionPlan::RESEARCH_PLAN
      ],
      SubscriptionPlan::MINI_PLAN => [
        subscription.recurring? ? nil : SubscriptionPlan::MINI_PLAN,
        SubscriptionPlan::STANDARD_PLAN,
        SubscriptionPlan::PRO_PLAN,
        SubscriptionPlan::RESEARCH_PLAN
      ].compact,
      SubscriptionPlan::STANDARD_PLAN => [
        subscription.recurring? ? nil : SubscriptionPlan::STANDARD_PLAN,
        SubscriptionPlan::PRO_PLAN,
        SubscriptionPlan::RESEARCH_PLAN
      ].compact,
      SubscriptionPlan::PRO_PLAN => [
        subscription.recurring? ? nil : SubscriptionPlan::PRO_PLAN
      ],
      SubscriptionPlan::RESEARCH_PLAN => [
        SubscriptionPlan::MINI_PLAN,
        SubscriptionPlan::STANDARD_PLAN,
        SubscriptionPlan::PRO_PLAN,
        subscription.recurring? ? nil : SubscriptionPlan::RESEARCH_PLAN
      ]
    }
  end
end
