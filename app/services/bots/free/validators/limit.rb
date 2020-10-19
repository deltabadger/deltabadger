module Bots::Free::Validators
  class Limit < BaseService
    def call(user)
      return Result::Success.new if user.unlimited? || user.first_month?

      if user.limit_reached?
        Result::Failure.new('Free plan limit reached')
      else
        Result::Success.new
      end
    end
  end
end
