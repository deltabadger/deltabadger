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
      end_time = new_subscription_plan_variant.years.nil? ? nil : Time.current + new_subscription_plan_variant.duration

      begin
        Subscription.create!(
          user_id: user.id,
          subscription_plan_variant: new_subscription_plan_variant,
          end_time: end_time,
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
  end
end
