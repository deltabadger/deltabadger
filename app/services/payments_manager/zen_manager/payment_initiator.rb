module PaymentsManager
  module ZenManager
    class PaymentInitiator < BaseService
      def call(params, user)
        payment_result = PaymentsManager::PaymentCreator.call(params, 'zen')
        return payment_result if payment_result.failure?

        cost_data_result = PaymentsManager::CostDataCalculator.call(payment: payment_result.data, user: user)
        return cost_data_result if cost_data_result.failure?

        return Result::Failure.new unless payment_result.data.update(
          total: cost_data_result.data[:total_price],
          discounted: cost_data_result.data[:discount_percent].positive?,
          commission: cost_data_result.data[:commission]
        )

        payment_url_result = PaymentsManager::ZenManager::PaymentUrlGenerator.call(payment_result.data, user)
        return payment_url_result if payment_url_result.failure?

        if payment_result.data.update(
          payment_id: payment_url_result.data[:payment_url].split('/').last
        )
          payment_url_result
        else
          Result::Failure.new
        end
      end
    end
  end
end
