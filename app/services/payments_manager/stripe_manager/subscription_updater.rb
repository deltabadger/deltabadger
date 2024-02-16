module PaymentsManager
  module StripeManager
    class SubscriptionUpdater < ApplicationService
      def initialize(params, payment_intent, payment_intent_id)
        @params = params
        @payment_intent = payment_intent
        @payment_intent_id = payment_intent_id
      end

      def call
        payment_metadata = @payment_intent['metadata']
        cost_presenter = case subscription_plan_repository.find(payment_metadata['subscription_plan_id']).name
                         when 'hodler'
                           @params[:cost_presenters][payment_metadata['country']][:hodler]
                         when 'legendary_badger'
                           @params[:cost_presenters][payment_metadata['country']][:legendary_badger]
                         else
                           @params[:cost_presenters][payment_metadata['country']][:investor]
                         end
        payment_params = payment_metadata.to_hash.merge(@params)
        payment = PaymentsManager::StripeManager::PaymentCreator.new.stripe_payment(
          payment_params,
          cost_presenter.discount_percent_amount.to_f.positive?
        )
        UpgradeSubscription.call(payment_metadata['user_id'], payment_metadata['subscription_plan_id'], nil, payment.id)
      rescue StandardError => e
        Raven.capture_exception(e)
      end
    end
  end
end
