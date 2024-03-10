module PaymentsManager
  module StripeManager
    class PaymentFinalizer < BaseService
      STRIPE_SUCCEEDED_STATUS = %w[succeeded].freeze
      STRIPE_IN_PROCESS_STATUS = %w[requires_confirmation requires_action processing].freeze

      def initialize
        @notifications = Notifications::Subscription.new
      end

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

        update_params = {
          total: @payment_intent['amount'].to_i / 100,
          discounted: payment_metadata['discounted'],
          commission: payment_metadata['commission'],
          status: :paid,
          paid_at: Time.current
        }
        unless payment_result.data.update(update_params)
          return Result::Failure.new(payment_result.errors.full_messages.join(', '), data: update_params)
        end

        @notifications.invoice(payment: payment_result.data)

        PaymentsManager::SubscriptionUpgrader.call(payment_result.data.id)
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
