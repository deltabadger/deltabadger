module PaymentsManager
  module BtcpayManager
    class PaymentCreator < BaseService
      CURRENCY_EU         = ENV.fetch('PAYMENT_CURRENCY__EU').freeze
      CURRENCY_OTHER      = ENV.fetch('PAYMENT_CURRENCY__OTHER').freeze

      def initialize
        @client = PaymentsManager::BtcpayManager::BtcpayClient.new
        @payments_repository = PaymentsRepository.new
      end

      def call(params, cost_data)
        order_id = PaymentsManager::NextPaymentIdGetter.call
        payment = Payment.new(params.merge(id: order_id, payment_type: 'bitcoin'))
        user = params.fetch(:user)
        validation_result = validate_payment(payment)
        return validation_result if validation_result.failure?

        payment_result = create_payment(payment, user, cost_data)
        if payment_result.success?
          crypto_total = payment_result.data[:crypto_total]
          @payments_repository.create(
            payment_result.data.slice(:payment_id, :status, :external_statuses, :total, :crypto_total)
              .merge(
                id: order_id,
                currency: get_currency(payment),
                discounted: cost_data[:discount_percent].positive?,
                commission: cost_data[:commission],
                crypto_commission: get_crypto_commission(crypto_total, cost_data)
              )
              .merge(params)
          )
        end
        payment_result
      end

      private

      def validate_payment(payment)
        return Result::Success.new if payment.valid?

        Result.new(
          data: payment,
          errors: payment.errors.full_messages.push('User error')
        )
      end

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
