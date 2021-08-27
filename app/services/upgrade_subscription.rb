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

    @subscribe_plan.call(
      user: user,
      subscription_plan: subscription_plan,
      email_params: email_params
    )

    user.update(
      pending_wire_transfer: nil,
      pending_plan_id: nil
    )
  end
end
