module PaymentsManager
  class SubscriptionUpgrader < BaseService
    def call(payment)
      current_plan_name = payment.user.subscription.name
      new_plan_name = payment.subscription_plan.name

      plan_subscriber_result = PaymentsManager::PlanSubscriber.call(payment: payment)
      return plan_subscriber_result if plan_subscriber_result.failure?

      update_params = {
        pending_wire_transfer: nil,
        pending_plan_variant_id: nil
      }
      unless payment.user.update(update_params)
        return Result::Failure.new(payment.user.errors.full_messages.join(', '), data: update_params)
      end

      SendgridMailToList.new.change_plan_list(payment.user, current_plan_name, new_plan_name)

      Result::Success.new
    end
  end
end
