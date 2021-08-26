class SubscribePlan < BaseService
  def initialize(
    subscriptions_repository: SubscriptionsRepository.new,
    notifications: Notifications::Subscription.new
  )
    @subscriptions_repository = subscriptions_repository
    @notifications = notifications
  end

  def call(user:, subscription_plan:, email_params: nil, start_time: Time.current)
    @subscriptions_repository.create(
      user_id: user.id,
      subscription_plan_id: subscription_plan.id,
      end_time: start_time + subscription_plan.duration,
      credits: subscription_plan.credits
    )

    if wire_transfer?(email_params)
      @notifications.after_wire_transfer(
        user: user,
        subscription_plan: subscription_plan,
        name: email_params['name'],
        type: email_params['type'],
        amount: email_params['amount']
      )
    else
      @notifications.subscription_granted(user: user, subscription_plan: subscription_plan)
    end

    Result::Success.new
  end

  private

  def wire_transfer?(params)
    !params.nil?
  end
end
