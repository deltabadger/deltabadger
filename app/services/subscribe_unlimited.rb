class SubscribeUnlimited < BaseService
  def initialize(
    subscriptions_repository: SubscriptionsRepository.new,
    subscription_plans_repository: SubscriptionPlansRepository.new,
    notifications: Notifications::Subscription.new
  )

    @subscriptions_repository = subscriptions_repository
    @subscription_plans_repository = subscription_plans_repository
    @notifications = notifications
  end

  def call(user)
    subscription_plan =
      @subscription_plans_repository
      .find_by(name: 'unlimited')

    @subscriptions_repository.create(
      user_id: user.id,
      subscription_plan_id: subscription_plan.id,
      end_time: Time.now + 1.year,
      credits: 1000
    )

    @notifications.unlimited_granted(user: user)

    Result::Success.new
  end
end
