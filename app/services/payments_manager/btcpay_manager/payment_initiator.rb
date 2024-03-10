require 'utilities/number'

module PaymentsManager
  module BtcpayManager
    class PaymentInitiator < BaseService
      def call(params)
        payment_result = PaymentsManager::PaymentCreator.call(params, 'bitcoin')
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

        invoice_result = PaymentsManager::BtcpayManager::InvoiceCreator.call(payment_result.data, params[:user])
        return invoice_result if invoice_result.failure?

        crypto_total = invoice_result.data[:crypto_total]
        update_params = {
          payment_id: invoice_result.data[:payment_id],
          status: invoice_result.data[:status],
          external_statuses: invoice_result.data[:external_statuses],
          crypto_total: crypto_total,
          crypto_commission: get_crypto_commission(crypto_total, cost_data_result.data)
        }
        unless payment_result.data.update(update_params)
          return Result::Failure.new(payment_result.errors.full_messages.join(', '), data: update_params)
        end

        invoice_result
      end

      private

      def get_crypto_commission(crypto_total, cost_data)
        crypto_total_price = Utilities::Number.to_bigdecimal(crypto_total, precision: 8)
        return 0 if crypto_total_price.zero?

        btc_price = cost_data[:total_price] / crypto_total_price
        return 0 if btc_price.zero?

        (cost_data[:commission] / btc_price).round(8, BigDecimal::ROUND_DOWN)
      end
    end
  end
end
