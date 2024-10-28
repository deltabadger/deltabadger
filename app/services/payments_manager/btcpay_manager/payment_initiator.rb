require 'utilities/number'

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
          btc_total: btc_total,
          btc_commission: get_btc_commission(btc_total, payment)
        }
        unless payment.update(update_params)
          return Result::Failure.new('ActiveRecord error', data: update_params)
        end

        invoice_result
      end

      private

      def get_btc_commission(btc_total, payment)
        crypto_price_with_vat = Utilities::Number.to_bigdecimal(btc_total, precision: 8)
        return 0 if crypto_price_with_vat.zero?

        btc_price = payment.price_with_vat / crypto_price_with_vat
        return 0 if btc_price.zero?

        (payment.referrer_commission_amount / btc_price).round(8, BigDecimal::ROUND_DOWN)
      end
    end
  end
end
