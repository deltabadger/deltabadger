module PaymentsManager
  module ZenManager
    class PaymentInitiator < BaseService
      def call(payment)
        payment_url_result = PaymentsManager::ZenManager::PaymentUrlCreator.call(payment, payment.user)
        return payment_url_result if payment_url_result.failure?

        update_params = {
          payment_id: payment_url_result.data[:payment_url].split('/').last
        }
        return Result::Failure.new('ActiveRecord error', data: update_params) unless payment.update(update_params)

        payment_url_result
      end
    end
  end
end
