module Bots::Free::Validators
  class Limit < BaseService
    def call(user)
      # Result::Failure.new
      Result::Success.new
    end
  end
end
