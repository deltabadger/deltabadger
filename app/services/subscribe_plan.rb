class SubscribePlan < BaseService
  def initialize(
    subscriptions_repository: SubscriptionsRepository.new,
    notifications: Notifications::Subscription.new
  )
    @subscriptions_repository = subscriptions_repository
    @notifications = notifications
  end

  def call(user:, subscription_plan:, name: nil)
    @subscriptions_repository.create(
      user_id: user.id,
      subscription_plan_id: subscription_plan.id,
      end_time: Time.current + subscription_plan.duration,
      credits: subscription_plan.credits
    )

    if wire_transfer?(name)
      @notifications.after_wire_transfer(
        user: user,
        subscription_plan: subscription_plan,
        name: name
      )
    else
      @notifications.subscription_granted(user: user, subscription_plan: subscription_plan)
    end

    Result::Success.new
  end

  private

  def wire_transfer?(name)
    !name.nil?
  end
end
