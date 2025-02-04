module PaymentsManager
  module BtcpayManager
    class PaymentInitiator < BaseService
      def call(payment)
        invoice_result = PaymentsManager::BtcpayManager::InvoiceCreator.call(payment)
        return invoice_result if invoice_result.failure?

        btc_total = invoice_result.data[:btc_total]
        update_params = {
          payment_id: invoice_result.data[:payment_id],
          status: invoice_result.data[:status],
          external_statuses: invoice_result.data[:external_statuses],
          btc_total: btc_total
        }
        return Result::Failure.new('ActiveRecord error', data: update_params) unless payment.update(update_params)

        invoice_result
      end
    end
  end
end
