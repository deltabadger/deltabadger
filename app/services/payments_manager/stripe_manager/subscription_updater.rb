module PaymentsManager
  module StripeManager
    class SubscriptionUpdater < BaseService
      def call(payment_intent, user, cost_data)
        payment_metadata = payment_intent['metadata']
        payment = PaymentsManager::StripeManager::PaymentCreator.call(
          payment_metadata.to_h,
          user,
          cost_data
        )
        UpgradeSubscription.call(payment_metadata['user_id'], payment_metadata['subscription_plan_id'], nil, payment.id)
      rescue StandardError => e
        Raven.capture_exception(e)
      end
    end
  end
end
