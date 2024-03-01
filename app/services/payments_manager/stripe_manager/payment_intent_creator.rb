module PaymentsManager
  module StripeManager
    class PaymentIntentCreator < BaseService
      def call(params, user, cost_data)
        # We create a fake payment to calculate the costs of the transactions
        fake_payment = Payment.new(country: params[:country], subscription_plan_id: params[:subscription_plan_id])

        metadata = {
          user_id: user[:id],
          email: user[:email],
          subscription_plan_id: params[:subscription_plan_id],
          country: params[:country]
        }
        Stripe::PaymentIntent.create(
          amount: amount_in_cents(cost_data[:total_price]),
          currency: fake_payment.eu? ? 'eur' : 'usd',
          metadata: metadata
        )
      end

      private

      # FIXME: use generic amount_in_cents method (helper?)
      def amount_in_cents(amount)
        (amount * 100).round
      end
    end
  end
end
