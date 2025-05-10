module PaymentsManager
  class SubscriptionUpgrader < BaseService
    def call(payment)
      user = payment.user
      previous_plan_name = user.subscription.name
      new_plan_name = payment.subscription_plan.name

      plan_subscriber_result = PaymentsManager::PlanSubscriber.call(payment: payment)
      return plan_subscriber_result if plan_subscriber_result.failure?

      update_params = {
        pending_wire_transfer: nil,
        pending_plan_variant_id: nil
      }
      return Result::Failure.new(user.errors.full_messages.join(', '), data: update_params) unless user.update(update_params)

      user.change_sendgrid_plan_list(previous_plan_name, new_plan_name) # TODO: move this to Subscription after_create

      Result::Success.new
    end
  end
end
