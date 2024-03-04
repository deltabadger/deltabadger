module PaymentsManager
  module StripeManager
    class PaymentFinalizer < BaseService
      STRIPE_SUCCEEDED_STATUS = %w[succeeded].freeze
      STRIPE_IN_PROCESS_STATUS = %w[requires_confirmation requires_action processing].freeze

      def call(params)
        @payment_intent = Stripe::PaymentIntent.retrieve(params['payment_intent_id'])
        return Result::Failure.new('Payment in process') if stripe_payment_in_process?
        return Result::Failure.new('Payment failed') unless stripe_payment_succeeded?

        payment = Payment.find_by(payment_id: params['payment_intent_id'])

        update_params = {
          status: :paid,
          paid_at: Time.current
        }

        return Result::Failure.new unless payment.update(update_params)

        payment_metadata = @payment_intent['metadata']
        UpgradeSubscription.call(payment_metadata['user_id'], payment_metadata['subscription_plan_id'], nil, payment.id)
      rescue StandardError => e
        Result::Failure.new(e.message)
      end

      private

      def stripe_payment_succeeded?
        @payment_intent['status'].in? STRIPE_SUCCEEDED_STATUS
      end

      def stripe_payment_in_process?
        @payment_intent['status'].in? STRIPE_IN_PROCESS_STATUS
      end
    end
  end
end
