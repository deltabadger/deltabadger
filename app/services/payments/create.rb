module Payments
  class Create < BaseService
    CURRENCY_EU = ENV.fetch('PAYMENT_CURRENCY__EU').freeze
    CURRENCY_OTHER = ENV.fetch('PAYMENT_CURRENCY__OTHER').freeze
    PAYMENT_SEQUENCE_ID = "'payments_id_seq'".freeze

    def initialize(
      client: Payments::Client.new,
      payments_repository: PaymentsRepository.new,
      payment_validator: Payments::Validators::Create.new,
      cost_calculator_class: Payments::CostCalculator
    )
      @client = client
      @payments_repository = payments_repository
      @payment_validator = payment_validator
      @cost_calculator_class = cost_calculator_class
    end

    def call(params)
      order_id = get_sequenced_id
      payment = Payment.new(params.merge(id: order_id))
      user = params.fetch(:user)
      validation_result = @payment_validator.call(payment)

      return validation_result if validation_result.failure?

      cost_calculator = get_cost_calculator(payment, user)
      payment_result = create_payment(payment, user, cost_calculator)
      if payment_result.success?
        crypto_total = payment_result.data[:crypto_total].nil? ? 0.0 : payment_result.data[:crypto_total]
        @payments_repository.create(
          payment_result.data.slice(:payment_id, :status, :external_statuses, :total)
            .merge(
              id: order_id,
              currency: currency(payment),
              discounted: cost_calculator.discount_percent.positive?,
              commission: cost_calculator.commission,
              crypto_total: crypto_total,
              crypto_commission: cost_calculator.crypto_commission(crypto_total_price: crypto_total)
            )
            .merge(params)
        )
      end
      payment_result
    end

    # create payment when wire transfer
    def wire_transfer(params, discounted)
      cost_calculator = get_wire_cost_calculator(params)
      total = cost_calculator.total_price
      currency = params['country'] == 'Other' ? 0 : 1 # 0 is for USD and 1 is for EUR. All people outside Europe get their prices in USD
      @payments_repository.create(
        id: get_sequenced_id,
        status: :pending,
        user: params[:user],
        first_name: params[:first_name],
        last_name: params[:last_name],
        country: params[:country],
        subscription_plan_id: params[:subscription_plan_id],
        birth_date: Time.now.strftime('%d/%m/%Y'),
        discounted: discounted,
        wire_transfer: true,
        total: total,
        currency: currency
      )
    end

    # HACK: It is needed to know the new record id before creating it
    def get_sequenced_id
      ActiveRecord::Base.connection.execute("SELECT nextval(#{PAYMENT_SEQUENCE_ID})")[0]['nextval']
    end

    private

    def create_payment(payment, user, cost_calculator)
      @client.create_payment(
        price: cost_calculator.total_price.to_s,
        currency: currency(payment),
        email: user.email,
        order_id: payment.id,
        name: "#{payment.first_name} #{payment.last_name}",
        country: payment.country,
        item_description: SubscriptionPlan.find(payment.subscription_plan_id).name.capitalize + ' Plan Upgrade',
        birth_date: payment.birth_date
      )
    end

    def currency(payment)
      payment.eu? ? CURRENCY_EU : CURRENCY_OTHER
    end

    def get_cost_calculator(payment, user)
      subscription_plan = payment.subscription_plan
      referrer = user.eligible_referrer
      discount_percent = referrer&.discount_percent || 0
      commission_percent = referrer&.commission_percent || 0

      current_plan = user.subscription.subscription_plan

      vat = VatRate.find_by!(country: payment.country).vat

      Payments::CostCalculatorFactory.call(
        eu: payment.eu?,
        vat: vat,
        subscription_plan: subscription_plan,
        current_plan: current_plan,
        days_left: user.plan_days_left,
        discount_percent: discount_percent,
        commission_percent: commission_percent
      )
    end

    def get_wire_cost_calculator(params)
      params['cost_presenters'][params['country']][SubscriptionPlan.find(params['subscription_plan_id']).name].cost_calculator
    end
  end
end
