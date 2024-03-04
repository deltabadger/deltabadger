module PaymentsManager
  module WireManager
    class PaymentInitiator < BaseService
      def call(params, user)
        payment_result = PaymentsManager::PaymentCreator.call(params, 'wire')
        return payment_result if payment_result.failure?

        cost_data_result = PaymentsManager::CostDataCalculator.call(payment: payment_result.data, user: user)
        return cost_data_result if cost_data_result.failure?

        return Result::Failure.new unless payment_result.data.update(
          total: cost_data_result.data[:total_price],
          discounted: cost_data_result.data[:discount_percent].positive?,
          commission: cost_data_result.data[:commission],
          status: :pending
        )

        Result::Success.new(payment)
      end
    end
  end
end
