module PaymentsManager
  module BtcpayManager
    class PaymentCreator < ApplicationService
      CURRENCY_EU         = ENV.fetch('PAYMENT_CURRENCY__EU').freeze
      CURRENCY_OTHER      = ENV.fetch('PAYMENT_CURRENCY__OTHER').freeze

      def initialize(params)
        @params = params
        @client = PaymentsManager::BtcpayManager::BtcpayClient.new
        @payments_repository = PaymentsRepository.new
        @cost_calculator_class = PaymentsManager::CostCalculator
      end

      def call
        order_id = PaymentsManager::NextPaymentIdGetter.call
        payment = Payment.new(@params.merge(id: order_id, payment_type: 'bitcoin'))
        user = @params.fetch(:user)
        validation_result = validate_payment(payment)

        return validation_result if validation_result.failure?

        cost_calculator = PaymentsManager::CostCalculatorGetter.call(payment: payment, user: user)

        payment_result = create_payment(payment, user, cost_calculator)
        if payment_result.success?
          crypto_total = payment_result.data[:crypto_total]
          puts "payment_result.data: #{payment_result.data.inspect}"
          @payments_repository.create(
            payment_result.data.slice(:payment_id, :status, :external_statuses, :total, :crypto_total)
              .merge(
                id: order_id,
                currency: get_currency(payment),
                discounted: cost_calculator.discount_percent.positive?,
                commission: cost_calculator.commission,
                crypto_commission: cost_calculator.crypto_commission(crypto_total_price: crypto_total)
              )
              .merge(@params)
          )
        end
        payment_result
      end

      private

      def validate_payment(payment)
        return Result::Success.new if payment.valid?

        Result.new(
          data: payment,
          errors: payment.errors.full_messages.push('user error')
        )
      end

      def create_payment(payment, user, cost_calculator)
        @client.create_payment(
          price: cost_calculator.total_price.to_s,
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
    end
  end
end
