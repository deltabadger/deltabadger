module PaymentsManager
  module ZenManager
    class PaymentCreator < BaseService
      CURRENCY_EU         = ENV.fetch('PAYMENT_CURRENCY__EU').freeze
      CURRENCY_OTHER      = ENV.fetch('PAYMENT_CURRENCY__OTHER').freeze

      def initialize
        @payments_repository = PaymentsRepository.new
      end

      def call(params)
        payment_result = PaymentsManager::NextPaymentCreator.call(params, 'zen')
        return payment_result if payment_result.failure?

        user = params.fetch(:user)
        cost_data_result = PaymentsManager::CostDataCalculator.call(payment: payment_result.data, user: user)
        return cost_data_result if cost_data_result.failure?

        total = cost_data_result.data[:total_price]
        payment_url_result = PaymentsManager::ZenManager::PaymentUrlGenerator.call(
          price: total.to_s,
          currency: get_currency(payment_result.data),
          email: user.email,
          order_id: payment_result.data.id,
          country: payment_result.data.country,
          item_description: get_item_description(payment_result.data)
        )
        return payment_url_result if payment_url_result.failure?

        payment_result.data.update(
          payment_id: payment_url_result.data[:payment_url].split('/').last,
          status: :unpaid,
          total: total,
          currency: get_currency(payment_result.data),
          discounted: cost_data_result.data[:discount_percent].positive?,
          commission: cost_data_result.data[:commission]
        )
        if payment_result.data.save
          payment_url_result
        else
          Result::Failure.new
        end
      end

      private

      def get_currency(payment)
        payment.eu? ? CURRENCY_EU : CURRENCY_OTHER
      end

      def get_item_description(payment)
        "#{SubscriptionPlan.find(payment.subscription_plan_id).name.capitalize} Plan Upgrade"
      end
    end
  end
end
