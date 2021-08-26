class UpgradeSubscription < BaseService
  def initialize(
    subscribe_plan = SubscribePlan.new,
    subscriptions_repository = SubscriptionPlansRepository.new
  )
    @subscribe_plan = subscribe_plan
    @subscriptions_repository = subscriptions_repository
  end

  def call(user_id, subscription_plan_id, email_params)
    user = User.find(user_id)
    subscription_plan = @subscriptions_repository.find(subscription_plan_id)
    start_time = start_time(user.subscription, subscription_plan_id)

    @subscribe_plan.call(
      user: user,
      subscription_plan: subscription_plan,
      email_params: email_params,
      start_time: start_time
    )

    user.update(
      pending_wire_transfer: nil,
      pending_plan_id: nil
    )
  end

  def start_time(current_subscription, subscription_plan_id)
    return current_subscription.end_time if current_subscription.id == subscription_plan_id

    Time.current
  end
end
