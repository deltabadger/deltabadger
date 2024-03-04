module PaymentsManager
  class PaymentCreator < BaseService
    CURRENCY_EU         = ENV.fetch('PAYMENT_CURRENCY__EU').freeze
    CURRENCY_OTHER      = ENV.fetch('PAYMENT_CURRENCY__OTHER').freeze
    PAYMENT_SEQUENCE_ID = "'payments_id_seq'".freeze

    def call(payment_params, payment_type)
      payment = Payment.new(payment_params.merge(id: get_next_payment_id, payment_type: payment_type))
      validation_result = validate_payment(payment)
      return validation_result if validation_result.failure?

      if payment.update(
        status: :unpaid,
        currency: get_currency(payment)
      )
        Result::Success.new(payment)
      else
        Result::Failure.new
      end
    end

    private

    def validate_payment(payment)
      if payment.valid?
        Result::Success.new
      else
        Result::Failure.new(payment.errors.full_messages.push('User error'), data: payment)
      end
    end

    def get_next_payment_id
      # HACK: It is needed to know the new record id before creating it
      ActiveRecord::Base.connection.execute("SELECT nextval(#{PAYMENT_SEQUENCE_ID})").first['nextval']
    end

    def get_currency(payment)
      payment.eu? ? CURRENCY_EU : CURRENCY_OTHER
    end
  end
end
