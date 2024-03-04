module PaymentsManager
  module BtcpayManager
    class PaymentInitiator < BaseService
      def initialize
        @client = PaymentsManager::BtcpayManager::BtcpayClient.new
      end

      def call(params, user)
        payment_result = PaymentsManager::NextPaymentCreator.call(params, 'bitcoin')
        return payment_result if payment_result.failure?

        cost_data_result = PaymentsManager::CostDataCalculator.call(payment: payment_result.data, user: user)
        return cost_data_result if cost_data_result.failure?

        return Result::Failure.new unless payment_result.data.update(
          total: cost_data_result.data[:total_price],
          discounted: cost_data_result.data[:discount_percent].positive?,
          commission: cost_data_result.data[:commission]
        )

        btcpay_payment_result = create_payment(payment_result.data, user)
        return btcpay_payment_result if btcpay_payment_result.failure?

        crypto_total = btcpay_payment_result.data[:crypto_total]
        if payment_result.data.update(
          payment_id: btcpay_payment_result.data[:payment_id],
          status: btcpay_payment_result.data[:status],
          external_statuses: btcpay_payment_result.data[:external_statuses],
          crypto_total: crypto_total,
          crypto_commission: get_crypto_commission(crypto_total, cost_data_result.data)
        )
          btcpay_payment_result
        else
          Result::Failure.new
        end
      end

      private

      def create_payment(payment, user)
        @client.create_payment(
          price: payment.total.to_s,
          currency: payment.currency,
          email: user.email,
          order_id: payment.id,
          name: "#{payment.first_name} #{payment.last_name}",
          country: payment.country,
          item_description: "#{SubscriptionPlan.find(payment.subscription_plan_id).name.capitalize} Plan Upgrade",
          birth_date: payment.birth_date
        )
      end

      def get_crypto_commission(crypto_total, cost_data)
        crypto_total_price = to_bigdecimal(crypto_total, precision: 8)
        crypto_without_vat = crypto_total_price / (1 + cost_data[:vat])
        crypto_base_price = crypto_without_vat / (1 - cost_data[:discount_percent])
        (crypto_base_price * cost_data[:commission_percent]).round(8, BigDecimal::ROUND_DOWN)
      end

      # FIXME: use generic to_bigdecimal method (helper?)
      def to_bigdecimal(num, precision: 2)
        BigDecimal(format("%0.0#{precision}f", num))
      end
    end
  end
end
