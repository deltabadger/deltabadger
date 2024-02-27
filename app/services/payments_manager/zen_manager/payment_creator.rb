module PaymentsManager
  module ZenManager
    class PaymentCreator < BaseService
      CURRENCY_EU         = ENV.fetch('PAYMENT_CURRENCY__EU').freeze
      CURRENCY_OTHER      = ENV.fetch('PAYMENT_CURRENCY__OTHER').freeze

      def initialize
        @payments_repository = PaymentsRepository.new
      end

      def call(params)
        order_id = PaymentsManager::NextPaymentIdGetter.call.data
        puts "order_id: #{order_id}"
        payment = Payment.new(params.merge(id: order_id, payment_type: 'zen'))
        user = params.fetch(:user)
        validation_result = validate_payment(payment)

        puts "payment: #{payment.inspect}"

        return validation_result if validation_result.failure?

        cost_data_result = PaymentsManager::CostDataCalculator.call(
          from_eu: payment.eu?,
          vat: VatRate.find_by!(country: payment.country).vat,
          subscription_plan: payment.subscription_plan,
          user: user
        )
        return cost_data_result if cost_data_result.failure?

        total = cost_data_result.data[:total_price]

        payment_url_result = PaymentsManager::ZenManager::PaymentUrlGenerator.call(
          price: total.to_s,
          currency: get_currency(payment),
          email: user.email,
          order_id: payment.id,
          first_name: payment.first_name,
          last_name: payment.last_name,
          country: payment.country,
          item_description: "#{SubscriptionPlan.find(payment.subscription_plan_id).name.capitalize} Plan Upgrade" # TODO: move to ItemDescriptionCreator?
        )
        return payment_url_result if payment_url_result.failure?

        if payment_url_result.success?
          @payments_repository.create(
            params.merge(
              id: order_id,
              status: :unpaid,
              payment_type: 'zen',
              total: total,
              currency: get_currency(payment),
              discounted: cost_data_result.data[:discount_percent].positive?,
              commission: cost_data_result.data[:commission]
            )
          )
        end
        payment_url_result
      end

      private

      def validate_payment(payment)
        if payment.valid?
          Result::Success.new
        else
          Result::Failure.new(payment.errors.full_messages.push('User error'), data: payment)
        end
      end

      def get_currency(payment)
        payment.eu? ? CURRENCY_EU : CURRENCY_OTHER
      end
    end
  end
end
