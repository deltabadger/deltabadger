module PaymentsManager
  module WireManager
    class PaymentFinalizer < BaseService
      def initialize
        @notifications = Notifications::Subscription.new
        @fomo_notifications = Notifications::FomoEvents.new
      end

      def call(payment, user)
        UpgradeSubscriptionWorker.perform_at(
          15.minutes.since(Time.current),
          user.id,
          payment.subscription_plan_id,
          email_params,
          payment.id
        )

        @notifications.wire_transfer_summary(
          email: user.email,
          subscription_plan: SubscriptionPlan.find(payment.subscription_plan_id).name,
          first_name: payment.first_name,
          last_name: payment.last_name,
          country: payment.country,
          amount: format('%0.02f', payment.total)
        )

        return Result::Failure.new unless user.update(
          pending_wire_transfer: payment.country,
          pending_plan_id: payment.subscription_plan_id
        )

        @fomo_notifications.plan_bought(
          first_name: payment.first_name,
          ip_address: request.remote_ip,
          plan_name: SubscriptionPlan.find(payment.subscription_plan_id).name
        )

        Result::Success.new
      end

      private

      def email_params(payment)
        {
          name: payment.first_name,
          type: payment.country,
          amount: format('%0.02f', payment.total)
        }
      end
    end
  end
end
