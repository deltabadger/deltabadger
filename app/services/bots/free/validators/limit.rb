module Bots::Free::Validators
  class Limit < BaseService
    def call(user)
      if user.credits <= 0
        Result::Failure.new('Limit reached')
      else
        Result::Success.new
      end
    end
  end
end
