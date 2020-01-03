module Bots::Free::Validators
  class Limit < BaseService
    def call(user)
      return Result::Success.new if user.unlimited?

      if user.limit_reached?
        Result::Failure.new('Credits limit reached')
      else
        Result::Success.new
      end
    end
  end
end
