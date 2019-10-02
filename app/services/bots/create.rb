module Bots
  class Create < BaseService
    def initialize(
      subscription_validator: -> { Result::Success.new }
    )
      @subscription_validator = subscription_validator
    end

    def call(user, params)
      subscription_validation_result = @subscription_validator.call
      if subscription_validation_result.failure?
        return subscription_validation_result
      end

      bot_params = params.merge(user: user)
      type = params.fetch(:bot_type)
      case type
      when 'free'
        Bots::CreateBot.new(
          bot_validator: Bots::Free::Validator.new,
          format_params: Bots::Free::FormatParams.new
        ).call(bot_params)
      else
        Result::Failure.new('Invalid bot type')
      end
    end
  end
end
