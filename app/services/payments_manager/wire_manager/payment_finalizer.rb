module PaymentsManager
  module WireManager
    class PaymentFinalizer < BaseService
      def initialize
        @notifications = Notifications::Subscription.new
      end

      def call(payment)
        @notifications.wire_transfer_summary(
          id: payment.id,
          email: payment.user.email,
          subscription_plan: payment.subscription_plan.name,
          first_name: payment.first_name,
          last_name: payment.last_name,
          country: payment.country,
          amount: format('%0.02f', payment.total)
        )

        update_params = {
          pending_wire_transfer: payment.country,
          pending_plan_id: payment.subscription_plan_id
        }
        unless payment.user.update(update_params)
          return Result::Failure.new(payment.user.errors.full_messages.join(', '), data: update_params)
        end

        UpgradeSubscriptionWorker.perform_at(
          15.minutes.since(Time.current),
          payment.id,
          email_params(payment)
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
