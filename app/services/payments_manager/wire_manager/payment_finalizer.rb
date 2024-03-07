module PaymentsManager
  module WireManager
    class PaymentFinalizer < BaseService
      def initialize
        @notifications = Notifications::Subscription.new
      end

      def call(params)
        payment_result = PaymentsManager::PaymentCreator.call(params, 'wire')
        return payment_result if payment_result.failure?

        cost_data_result = PaymentsManager::CostDataCalculator.call(payment: payment_result.data, user: params[:user])
        return cost_data_result if cost_data_result.failure?

        return Result::Failure.new unless payment_result.data.update(
          total: cost_data_result.data[:total_price],
          discounted: cost_data_result.data[:discount_percent].positive?,
          commission: cost_data_result.data[:commission]
        )

        @notifications.wire_transfer_summary(
          id: payment_result.data.id,
          email: params[:user].email,
          subscription_plan: SubscriptionPlan.find(payment_result.data.subscription_plan_id).name,
          first_name: payment_result.data.first_name,
          last_name: payment_result.data.last_name,
          country: payment_result.data.country,
          amount: format('%0.02f', payment_result.data.total)
        )

        return Result::Failure.new unless params[:user].update(
          pending_wire_transfer: payment_result.data.country,
          pending_plan_id: payment_result.data.subscription_plan_id
        )

        UpgradeSubscriptionWorker.perform_at(
          15.minutes.since(Time.current),
          payment_result.data.id,
          email_params(payment_result.data)
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
