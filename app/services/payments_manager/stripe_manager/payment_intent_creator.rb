module PaymentsManager
  module StripeManager
    class PaymentIntentCreator < BaseService
      def call(params, user)
        payment_params = {
          subscription_plan_id: params[:subscription_plan_id],
          country: params[:country],
          user: user
        }
        payment_result = PaymentsManager::PaymentCreator.call(payment_params, 'stripe')
        return payment_result if payment_result.failure?

        cost_data_result = PaymentsManager::CostDataCalculator.call(payment: payment_result.data, user: user)
        return cost_data_result if cost_data_result.failure?

        return Result::Failure.new unless payment_result.data.update(
          total: cost_data_result.data[:total_price],
          discounted: cost_data_result.data[:discount_percent].positive?,
          commission: cost_data_result.data[:commission]
        )

        metadata = {
          user_id: user[:id],
          email: user[:email],
          subscription_plan_id: payment_result.data.subscription_plan_id,
          country: payment_result.data.country
        }
        payment_intent = Stripe::PaymentIntent.create(
          amount: amount_in_cents(payment_result.data.total),
          currency: payment_result.data.currency,
          metadata: metadata
        )
        Result::Success.new(payment_intent)
      rescue StandardError => e
        Result::Failure.new(e.message)
      end

      private

      # FIXME: use generic amount_in_cents method (helper?)
      def amount_in_cents(amount)
        (amount * 100).round
      end
    end
  end
end
