module Bots
  class CreateBot < BaseService
    def initialize(
      bot_validator:,
      format_params:
    )

      @bot_validator = bot_validator
      @format_params = format_params
    end

    def call(params)
      formatted_params = @format_params.call(params)
      bot = Bot.new(formatted_params)

      result = @bot_validator.call(bot, params[:user])

      if result.success?
        bot.save!
        Result::Success.new(bot)
      else
        Result::Failure.new(*result.errors)
      end
    end
  end
end
