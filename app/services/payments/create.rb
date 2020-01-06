module Payments
  class Create < BaseService
    COST_EU = ENV.fetch('UNLIMITED_SUBSCRYPTION_COST_AMOUNT__EU')
    COST_OTHER = ENV.fetch('UNLIMITED_SUBSCRYPTION_COST_AMOUNT__OTHER')
    CURRENCY = ENV.fetch('UNLIMITED_SUBSCRYPTION_COST_CURRENCY')

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
        currency: CURRENCY,
        email: user.email
      )

      if payment_result.success?
        @payments_repository.create(
          payment_result.data.slice(:payment_id, :status, :total)
          .merge(currency: CURRENCY).merge(params)
        )
      end

      payment_result
    end

    private

    def cost(payment)
      payment.eu? ? COST_EU : COST_OTHER
    end
  end
end
