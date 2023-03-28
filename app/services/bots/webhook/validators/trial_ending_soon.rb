module Bots::Webhook::Validators
  class TrialEndingSoon < BaseService
    def call(user)
      return Result::Success.new if user.unlimited? || !user.first_month?

      if user.created_at < (Date.current - 27.days)
        Result::Failure.new
      else
        Result::Success.new
      end
    end
  end
end
