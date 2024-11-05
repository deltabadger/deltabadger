module PaymentsManager
  module WireManager
    class PaymentFinalizer < BaseService
      def initialize
        @notifications = Notifications::Subscription.new
      end

      def call(payment)
        @notifications.wire_transfer_summary(payment: payment)

        update_params = {
          pending_wire_transfer: payment.country,
          pending_plan_variant_id: payment.subscription_plan_variant_id
        }
        return Result::Failure.new('ActiveRecord error', data: update_params) unless payment.user.update(update_params)

        UpgradeSubscriptionWorker.perform_at(15.minutes.since(Time.current), payment.id)

        Result::Success.new
      end
    end
  end
end
