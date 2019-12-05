module Payments
  class Create < BaseService
    COST = ENV['UNLIMITED_SUBSCRYPTION_COST_AMOUNT']
    CURRENCY = ENV['UNLIMITED_SUBSCRYPTION_COST_CURRENCY']

    def initialize(
      client: Payments::Client.new,
      payments_repository: PaymentsRepository.new
    )

      @client = client
      @payments_repository = payments_repository
    end

    def call(user)
      payment = @client.create_payment(
        price: COST,
        currency: CURRENCY,
        email: user.email
      )

      if payment.success?
        @payments_repository.create(
          payment.data.slice(:payment_id, :status, :total)
          .merge(currency: CURRENCY, user: user)
        )
      end
      payment
    end
  end
end
