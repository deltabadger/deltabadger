module PaymentsManager
  module StripeManager
    class PaymentFinalizer < BaseService
      STRIPE_SUCCEEDED_STATUS = %w[succeeded].freeze
      STRIPE_IN_PROCESS_STATUS = %w[requires_confirmation requires_action processing].freeze

      def call(params, user)
        @payment_intent = Stripe::PaymentIntent.retrieve(params[:payment_intent_id])
        return Result::Failure.new('Payment in process') if stripe_payment_in_process?
        return Result::Failure.new('Payment failed') unless stripe_payment_succeeded?

        payment_metadata = @payment_intent['metadata']

        payment_params = {
          subscription_plan_id: payment_metadata['subscription_plan_id'],
          country: payment_metadata['country'],
          user: user
        }
        payment_result = PaymentsManager::PaymentCreator.call(payment_params, 'stripe')
        return payment_result if payment_result.failure?

        return Result::Failure.new unless payment_result.data.update(
          total: @payment_intent['amount'].to_i / 100,
          discounted: payment_metadata['discounted'],
          commission: payment_metadata['commission'],
          status: :paid,
          paid_at: Time.current
        )

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
