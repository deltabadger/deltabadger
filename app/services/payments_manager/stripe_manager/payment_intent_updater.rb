module PaymentsManager
  module StripeManager
    class PaymentIntentUpdater < BaseService
      def call(params, cost_data)
        # We create a fake payment to calculate the costs of the transactions
        fake_payment = Payment.new(country: params[:country], subscription_plan_id: params[:subscription_plan_id])

        metadata = get_update_metadata(params)
        Stripe::PaymentIntent.update(params[:payment_intent_id],
                                     amount: amount_in_cents(cost_data[:total_price]),
                                     currency: fake_payment.eu? ? 'eur' : 'usd',
                                     metadata: metadata)
      end

      private

      def get_update_metadata(params)
        {
          country: params[:country],
          subscription_plan_id: params[:subscription_plan_id]
        }
      end

      # FIXME: use generic amount_in_cents method (helper?)
      def amount_in_cents(amount)
        (amount * 100).round
      end
    end
  end
end
