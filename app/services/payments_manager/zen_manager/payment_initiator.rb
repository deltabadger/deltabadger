module PaymentsManager
  module ZenManager
    class PaymentInitiator < BaseService
      def call(params)
        payment_result = PaymentsManager::PaymentCreator.call(params, 'zen')
        return payment_result if payment_result.failure?

        cost_data_result = PaymentsManager::CostDataCalculator.call(payment: payment_result.data, user: params[:user])
        return cost_data_result if cost_data_result.failure?

        update_params = {
          total: cost_data_result.data[:total_price],
          discounted: cost_data_result.data[:discount_percent].positive?,
          commission: cost_data_result.data[:commission]
        }
        unless payment_result.data.update(update_params)
          return Result::Failure.new(payment_result.errors.full_messages.join(', '), data: update_params)
        end

        payment_url_result = PaymentsManager::ZenManager::PaymentUrlCreator.call(payment_result.data, params[:user])
        return payment_url_result if payment_url_result.failure?

        update_params = {
          payment_id: payment_url_result.data[:payment_url].split('/').last
        }
        unless payment_result.data.update(update_params)
          return Result::Failure.new(payment_result.errors.full_messages.join(', '), data: update_params)
        end

        payment_url_result
      end
    end
  end
end
