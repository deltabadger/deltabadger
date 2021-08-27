module Payments::Validators
  class Create
    def call(payment)
      return Result::Success.new if payment.valid?

      Result.new(
        data: payment,
        errors: payment.errors.full_messages
      )
    end
  end
end
