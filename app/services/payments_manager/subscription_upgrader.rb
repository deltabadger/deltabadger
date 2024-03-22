module PaymentsManager
  class SubscriptionUpgrader < BaseService
    def initialize(
      fomo_notifications = Notifications::FomoEvents.new
    )
      @fomo_notifications = fomo_notifications
    end

    def call(payment_id, email_params = nil)
      payment = Payment.find(payment_id)
      current_plan_name = payment.user.subscription_name
      new_plan_name = payment.subscription_plan.name

      plan_subscriber_result = PaymentsManager::PlanSubscriber.call(
        user: payment.user,
        subscription_plan: payment.subscription_plan,
        email_params: email_params
      )
      return plan_subscriber_result if plan_subscriber_result.failure?

      SendgridMailToList.new.change_plan_list(payment.user, current_plan_name, new_plan_name)

      update_params = {
        pending_wire_transfer: nil,
        pending_plan_id: nil,
        welcome_banner_showed: true
      }
      unless payment.user.update(update_params)
        return Result::Failure.new(payment.user.errors.full_messages.join(', '), data: update_params)
      end

      # @fomo_notifications.plan_bought(
      #   first_name: payment.first_name,
      #   country: payment.country,
      #   plan_name: new_plan_name
      # )

      Result::Success.new
    end
  end
end
