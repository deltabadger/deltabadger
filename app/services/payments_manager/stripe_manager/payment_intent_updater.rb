module PaymentsManager
  module StripeManager
    class PaymentIntentUpdater < BaseService
      def call(params, user)
        # We create a fake payment to calculate the costs of the transactions
        fake_payment = Payment.new(country: params['country'], subscription_plan_id: params['subscription_plan_id'])
        cost_calculator = PaymentsManager::CostCalculatorGetter.call(payment: fake_payment, user: user)
        stripe_price = { total_price: cost_calculator.total_price }
        metadata = get_update_metadata(params)
        Stripe::PaymentIntent.update(params['payment_intent_id'],
                                     amount: amount_in_cents(stripe_price[:total_price]),
                                     currency: fake_payment.eu? ? 'eur' : 'usd',
                                     metadata: metadata)
      end

      private

      def get_update_metadata(params)
        {
          country: params['country'],
          subscription_plan_id: params['subscription_plan_id']
        }
      end

      def amount_in_cents(amount)
        (amount * 100).round
      end
    end
  end
end
