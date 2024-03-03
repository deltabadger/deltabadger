module PaymentsManager
  module BtcpayManager
    class PaymentCreator < BaseService
      CURRENCY_EU         = ENV.fetch('PAYMENT_CURRENCY__EU').freeze
      CURRENCY_OTHER      = ENV.fetch('PAYMENT_CURRENCY__OTHER').freeze

      def initialize
        @client = PaymentsManager::BtcpayManager::BtcpayClient.new
        @payments_repository = PaymentsRepository.new
      end

      def call(params)
        payment_result = PaymentsManager::NextPaymentCreator.call(params, 'bitcoin')
        return payment_result if payment_result.failure?

        user = params.fetch(:user)

        cost_data_result = PaymentsManager::CostDataCalculator.call(payment: payment_result.data, user: user)
        return cost_data_result if cost_data_result.failure?

        btcpay_payment_result = create_payment(payment_result.data, user, cost_data_result.data)
        return btcpay_payment_result if btcpay_payment_result.failure?

        crypto_total = btcpay_payment_result.data[:crypto_total]
        @payments_repository.create(
          btcpay_payment_result.data.slice(:payment_id, :status, :external_statuses, :total, :crypto_total)
            .merge(
              id: payment_result.data[:id],
              currency: get_currency(payment_result.data),
              discounted: cost_data_result.data[:discount_percent].positive?,
              commission: cost_data_result.data[:commission],
              crypto_commission: get_crypto_commission(crypto_total, cost_data_result.data)
            )
            .merge(params)
        )
        btcpay_payment_result
      end

      private

      def create_payment(payment, user, cost_data)
        @client.create_payment(
          price: cost_data[:total_price].to_s,
          currency: get_currency(payment),
          email: user.email,
          order_id: payment.id,
          name: "#{payment.first_name} #{payment.last_name}",
          country: payment.country,
          item_description: "#{SubscriptionPlan.find(payment.subscription_plan_id).name.capitalize} Plan Upgrade",
          birth_date: payment.birth_date
        )
      end

      def get_currency(payment)
        payment.eu? ? CURRENCY_EU : CURRENCY_OTHER
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
