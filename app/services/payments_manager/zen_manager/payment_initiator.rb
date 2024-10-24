module PaymentsManager
  module ZenManager
    class PaymentInitiator < BaseService
      def call(payment)
        payment_url_result = PaymentsManager::ZenManager::PaymentUrlCreator.call(payment, payment.user)
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
