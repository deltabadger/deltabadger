class SubscribePlan < BaseService
  def initialize(
    subscriptions_repository: SubscriptionsRepository.new,
    notifications: Notifications::Subscription.new
  )
    @subscriptions_repository = subscriptions_repository
    @notifications = notifications
  end

  def call(user:, subscription_plan:)
    @subscriptions_repository.create(
      user_id: user.id,
      subscription_plan_id: subscription_plan.id,
      end_time: Time.current + subscription_plan.years.to_i.years,
      credits: subscription_plan.credits
    )

    @notifications.subscription_granted(user: user, subscription_plan: subscription_plan)

    Result::Success.new
  end
end
