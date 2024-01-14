# TODO: need? -
module Bots::Webhook::Validators
  class AlmostLimit < BaseService
    def call(user)
      return Result::Success.new if user.unlimited? || user.first_month?

      if user.credits <= 100
        Result::Failure.new
      else
        Result::Success.new
      end
    end
  end
end
