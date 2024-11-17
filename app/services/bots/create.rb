module Bots
  class Create < BaseService
    def initialize(
      subscription_validator: -> { Result::Success.new }
    )
      @subscription_validator = subscription_validator
    end

    def call(user, params)
      subscription_validation_result = @subscription_validator.call
      return subscription_validation_result if subscription_validation_result.failure?

      bot_params = params.merge(user: user)
      type = params.fetch(:bot_type)
      case type
      when 'trading'
        Bots::CreateBot.new(
          bot_validator: Bots::Trading::Validators::Create.new,
          format_params: Bots::Trading::FormatParams::Create.new
        ).call(bot_params)
      when 'withdrawal'
        Bots::CreateBot.new(
          bot_validator: Bots::Withdrawal::Validators::Create.new,
          format_params: Bots::Withdrawal::FormatParams::Create.new
        ).call(bot_params)
      when 'webhook'
        Bots::CreateBot.new(
          bot_validator: Bots::Webhook::Validators::Create.new,
          format_params: Bots::Webhook::FormatParams::Create.new
        ).call(bot_params)
      else
        Result::Failure.new('Invalid bot type')
      end
    end
  end
end
