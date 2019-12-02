module Bots::Free::Validators
  class AlmostLimit < BaseService
    def call(user)
      return Result::Success.new if user.unlimited?

      if user.credits <= 200
        Result::Failure.new
      else
        Result::Success.new
      end
    end
  end
end
