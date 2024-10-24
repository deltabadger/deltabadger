module PaymentsManager
  class PlanSubscriber < BaseService
    def initialize(
      notifications: Notifications::Subscription.new
    )
      @notifications = notifications
    end

    def call(payment:)
      user = payment.user
      new_subscription_plan_variant = payment.subscription_plan_variant
      start_time = start_time(user.subscription, new_subscription_plan_variant)

      begin
        Subscription.create!(
          user_id: user.id,
          subscription_plan_variant: new_subscription_plan_variant,
          end_time: start_time + new_subscription_plan_variant.years,
          credits: new_subscription_plan_variant.subscription_plan.credits
        )
      rescue ActiveRecord::RecordInvalid => e
        return Result::Failure.new("Subscription could not be created: #{e.message}")
      end

      if payment.payment_type == 'wire'
        @notifications.after_wire_transfer(payment: payment)
      else
        @notifications.subscription_granted(payment: payment)
      end

      Result::Success.new
    end

    private

    def start_time(current_subscription, new_subscription)
      return current_subscription.end_time if current_subscription.name == new_subscription.name

      Time.current
    end
  end
end
