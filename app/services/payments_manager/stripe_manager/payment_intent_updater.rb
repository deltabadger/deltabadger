module PaymentsManager
  module StripeManager
    class PaymentIntentUpdater < BaseService
      def call(params)
        payment = Payment.find_by(payment_id: params[:payment_intent_id])

        metadata = get_update_metadata(params)
        payment_intent = Stripe::PaymentIntent.update(params[:payment_intent_id],
                                                      amount: amount_in_cents(payment.total),
                                                      currency: payment.currency,
                                                      metadata: metadata)
        Result::Success.new(payment_intent)
      rescue StandardError => e
        Result::Failure.new(e.message)
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
