module Bots::Webhook::Validators #TODO need? -
  class Limit < BaseService
    def call(user)
      return Result::Success.new if user.unlimited? || user.first_month?

      if user.limit_reached?
        Result::Failure.new(I18n.t('errors.limit_reached'))
      else
        Result::Success.new
      end
    end
  end
end
