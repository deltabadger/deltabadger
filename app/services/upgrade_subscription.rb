class UpgradeSubscription < BaseService
  def initialize(
    subscribe_plan = SubscribePlan.new,
    subscriptions_repository = SubscriptionPlansRepository.new
  )
    @subscribe_plan = subscribe_plan
    @subscriptions_repository = subscriptions_repository
  end

  def call(user_id, subscription_plan_id, email_params, payment_id)
    user = User.find(user_id)
    subscription_plan = @subscriptions_repository.find(subscription_plan_id)
    current_plan_name = user.subscription_name
    new_plan_name = subscription_plan.name

    @subscribe_plan.call(
      user: user,
      subscription_plan: subscription_plan,
      email_params: email_params
    )

    SendgridMailToList.new.change_plan_list(user, current_plan_name, new_plan_name)

    # unless payment_id.nil?
    #   payment = Payment.find(payment_id)
    #   payment.update(status: :paid)
    # end

    user.update(
      pending_wire_transfer: nil,
      pending_plan_id: nil,
      welcome_banner_showed: true
    )
  end
end
