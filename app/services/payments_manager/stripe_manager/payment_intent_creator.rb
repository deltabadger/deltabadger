module PaymentsManager
  module StripeManager
    class PaymentIntentCreator < BaseService
      def call(params, user)
        # We create a fake payment to calculate the costs of the transactions
        fake_payment = Payment.new(country: params['country'], subscription_plan_id: params['subscription_plan_id'])
        cost_calculator = PaymentsManager::CostCalculatorGetter.call(payment: fake_payment, user: user)
        stripe_price = { total_price: cost_calculator.total_price }
        metadata = {
          user_id: user['id'],
          email: user['email'],
          subscription_plan_id: params['subscription_plan_id'],
          country: params['country']
        }
        Stripe::PaymentIntent.create(
          amount: amount_in_cents(stripe_price[:total_price]),
          currency: fake_payment.eu? ? 'eur' : 'usd',
          metadata: metadata
        )
      end

      private

      def amount_in_cents(amount)
        (amount * 100).round
      end
    end
  end
end
