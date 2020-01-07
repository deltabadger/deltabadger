module Payments
  class Create < BaseService
    COST_EU = ENV.fetch('UNLIMITED_SUBSCRYPTION_COST_AMOUNT__EU')
    COST_OTHER = ENV.fetch('UNLIMITED_SUBSCRYPTION_COST_AMOUNT__OTHER')
    CURRENCY_EU = ENV.fetch('UNLIMITED_SUBSCRYPTION_COST_CURRENCY__EU')
    CURRENCY_OTHER = ENV.fetch('UNLIMITED_SUBSCRYPTION_COST_CURRENCY__OTHER')

    def initialize(
      client: Payments::Client.new,
      payments_repository: PaymentsRepository.new,
      payment_validator: Payments::Validators::Create.new
    )

      @client = client
      @payments_repository = payments_repository
      @payment_validator = payment_validator
    end

    def call(params)
      payment = Payment.new(params)
      validation_result = @payment_validator.call(payment)

      return validation_result if validation_result.failure?

      user = params.fetch(:user)
      payment_result = @client.create_payment(
        price: cost(payment),
        currency: currency(payment),
        email: user.email
      )

      if payment_result.success?
        @payments_repository.create(
          payment_result.data.slice(:payment_id, :status, :total)
          .merge(currency: currency(payment)).merge(params)
        )
      end

      payment_result
    end

    private

    def currency(payment)
      payment.eu? ? CURRENCY_EU : CURRENCY_OTHER
    end

    def cost(payment)
      payment.eu? ? COST_EU : COST_OTHER
    end
  end
end
