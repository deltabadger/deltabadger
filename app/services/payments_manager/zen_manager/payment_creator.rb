module PaymentsManager
  module ZenManager
    class PaymentCreator < ApplicationService
      CURRENCY_EU         = ENV.fetch('PAYMENT_CURRENCY__EU').freeze
      CURRENCY_OTHER      = ENV.fetch('PAYMENT_CURRENCY__OTHER').freeze
      PAYMENT_SEQUENCE_ID = "'payments_id_seq'".freeze

      def initialize(params)
        @params = params
        @client = PaymentsManager::ZenManager::ZenClient.new
        @payments_repository = PaymentsRepository.new
        @cost_calculator_class = PaymentsManager::CostCalculator
      end

      def call
        order_id = get_sequenced_id
        payment = Payment.new(@params.merge(id: order_id, payment_type: 'zen'))
        user = @params.fetch(:user)
        validation_result = validate_payment(payment)

        return validation_result if validation_result.failure?

        cost_calculator = get_cost_calculator(payment, user)
        total = cost_calculator.total_price

        payment_result = create_payment(payment, user, total)
        if payment_result.success?
          @payments_repository.create(
            @params.merge(
              id: order_id,
              status: :unpaid,
              payment_type: 'zen',
              total: total,
              currency: get_currency(payment),
              discounted: cost_calculator.discount_percent.positive?,
              commission: cost_calculator.commission,
              paid_at: Time.now.strftime('%d/%m/%Y')
            )
          )
        end
        payment_result
      end

      private

      # HACK: It is needed to know the new record id before creating it
      def get_sequenced_id
        ActiveRecord::Base.connection.execute("SELECT nextval(#{PAYMENT_SEQUENCE_ID})").first['nextval']
      end

      def validate_payment(payment)
        return Result::Success.new if payment.valid?

        Result.new(
          data: payment,
          errors: payment.errors.full_messages.push('user error')
        )
      end

      def create_payment(payment, user, total)
        @client.create_payment(
          price: total.to_s,
          currency: get_currency(payment),
          email: user.email,
          order_id: payment.id,
          first_name: payment.first_name,
          last_name: payment.last_name,
          country: payment.country,
          item_description: "#{SubscriptionPlan.find(payment.subscription_plan_id).name.capitalize} Plan Upgrade"
        )
      end

      def get_currency(payment)
        payment.eu? ? CURRENCY_EU : CURRENCY_OTHER
      end

      def get_cost_calculator(payment, user)
        subscription_plan = payment.subscription_plan
        referrer = user.eligible_referrer
        discount_percent = referrer&.discount_percent || 0
        commission_percent = referrer&.commission_percent || 0

        current_plan = user.subscription.subscription_plan

        vat = VatRate.find_by!(country: payment.country).vat

        PaymentsManager::CostCalculatorFactory.call(
          eu: payment.eu?,
          vat: vat,
          subscription_plan: subscription_plan,
          current_plan: current_plan,
          days_left: user.plan_days_left,
          discount_percent: discount_percent,
          commission_percent: commission_percent
        )
      end
    end
  end
end
