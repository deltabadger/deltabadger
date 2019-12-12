module Payments::Validators
  class Create
    def call(payment)
      if payment.valid?
        Result::Success.new
      else
        Result.new(
          data: payment,
          errors: payment.errors.full_messages
        )
      end
    end
  end
end
