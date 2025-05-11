module PaymentsManager
  class SubscriptionUpgrader < BaseService
    def call(payment)
      user = payment.user
      subscription_plan_variant = payment.subscription_plan_variant
      ends_at = subscription_plan_variant.years.nil? ? nil : Time.current + subscription_plan_variant.duration

      begin
        user.subscriptions.create!(
          subscription_plan_variant: subscription_plan_variant,
          ends_at: ends_at
        )
      rescue ActiveRecord::RecordInvalid => e
        return Result::Failure.new("Subscription could not be created: #{e.message}")
      end

      notifications = Notifications::Subscription.new
      if payment.payment_type == 'wire'
        notifications.after_wire_transfer(payment: payment)
      else
        notifications.subscription_granted(payment: payment)
      end

      update_params = {
        pending_wire_transfer: nil,
        pending_plan_variant_id: nil
      }
      return Result::Failure.new(user.errors.full_messages.join(', '), data: update_params) unless user.update(update_params)

      Result::Success.new
    end
  end
end
