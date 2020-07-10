module Payments
  class Create < BaseService
    COST_EU = ENV.fetch('UNLIMITED_SUBSCRYPTION_COST_AMOUNT__EU')
    COST_OTHER = ENV.fetch('UNLIMITED_SUBSCRYPTION_COST_AMOUNT__OTHER')
    CURRENCY_EU = ENV.fetch('UNLIMITED_SUBSCRYPTION_COST_CURRENCY__EU')
    CURRENCY_OTHER = ENV.fetch('UNLIMITED_SUBSCRYPTION_COST_CURRENCY__OTHER')
    VAT_EU = '0.2'.freeze
    VAT_OTHER = '0'.freeze

    def initialize(
      client: Payments::Client.new,
      payments_repository: PaymentsRepository.new,
      payment_validator: Payments::Validators::Create.new,
      cost_calculator: Payments::CostCalculator,
      commission_calculator: Payments::CommissionCalculator.new
    )
      @client = client
      @payments_repository = payments_repository
      @payment_validator = payment_validator
      @cost_calculator = cost_calculator
      @commission_calculator = commission_calculator
    end

    def call(params)
      payment = Payment.new(params)
      validation_result = @payment_validator.call(payment)

      return validation_result if validation_result.failure?

      user = params.fetch(:user)
      discount = user.referrer&.discount_percent || 0
      commission = user.referrer&.commission_percent || 0

      payment_result = @client.create_payment(
        price: cost(payment, discount).to_s,
        currency: currency(payment),
        email: user.email
      )

      if payment_result.success?
        crypto_total = payment_result.data[:crypto_total]
        @payments_repository.create(
          payment_result.data.slice(:payment_id, :status, :external_statuses, :total, :crypto_total)
          .merge(currency: currency(payment), **commission(payment, discount, commission, crypto_total)).merge(params)
        )
      end

      payment_result
    end

    private

    def currency(payment)
      payment.eu? ? CURRENCY_EU : CURRENCY_OTHER
    end

    def cost(payment, discount)
      calculator = if payment.eu?
                     @cost_calculator.new(base_price: COST_EU, vat: VAT_EU, discount_percent: discount)
                   else
                     @cost_calculator.new(base_price: COST_OTHER, vat: VAT_OTHER, discount_percent: discount)
                   end
      calculator.total_price.to_s
    end

    def commission(payment, discount, commission_percent, crypto_total_price)
      if payment.eu?
        @commission_calculator.call(base_price: COST_EU, vat: VAT_EU, discount: discount, commission_percent: commission_percent, crypto_total_price: crypto_total_price)
      else
        @commission_calculator.call(base_price: COST_OTHER, vat: VAT_OTHER, discount: discount, commission_percent: commission_percent, crypto_total_price: crypto_total_price)
      end
    end
  end
end
