module Bots::Free::Validators
  class Limit < BaseService
    def call(user)
      if user.limit_reached?
        Result::Failure.new('Limit reached')
      else
        Result::Success.new
      end
    end
  end
end
