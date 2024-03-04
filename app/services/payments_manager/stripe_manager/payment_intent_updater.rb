module PaymentsManager
  module StripeManager
    class PaymentIntentUpdater < BaseService
      def call(params, user)
        payment_params = {
          subscription_plan_id: params[:subscription_plan_id],
          country: params[:country],
          user: user
        }
        payment_result = PaymentsManager::PaymentCreator.call(payment_params, 'stripe', dry_run: true)
        return payment_result if payment_result.failure?

        cost_data_result = PaymentsManager::CostDataCalculator.call(payment: payment_result.data, user: user)
        return cost_data_result if cost_data_result.failure?

        metadata = get_update_metadata(params, cost_data_result.data)
        payment_intent = Stripe::PaymentIntent.update(
          params[:payment_intent_id],
          amount: amount_in_cents(cost_data_result.data[:total_price]),
          currency: payment_result.data.currency,
          metadata: metadata
        )
        Result::Success.new(payment_intent)
      rescue StandardError => e
        Result::Failure.new(e.message)
      end

      private

      def get_update_metadata(params, cost_data)
        {
          country: params[:country],
          subscription_plan_id: params[:subscription_plan_id],
          discounted: cost_data[:discount_percent].positive?,
          commission: cost_data[:commission]
        }
      end

      # FIXME: use generic amount_in_cents method (helper?)
      def amount_in_cents(amount)
        (amount * 100).round
      end
    end
  end
end
