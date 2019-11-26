module Bots::Free::Validators
  class AlmostLimit < BaseService
    def call(user)
      if user.credits <= 200
        Result::Failure.new
      else
        Result::Success.new
      end
    end
  end
end
