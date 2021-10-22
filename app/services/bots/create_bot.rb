module Bots
  class CreateBot < BaseService
    def initialize(
      bot_validator:,
      format_params:,
      bots_repository: BotsRepository.new
    )

      @bot_validator = bot_validator
      @format_params = format_params
      @bots_repository = bots_repository
    end

    def call(params)
      formatted_params = @format_params.call(params)
      bot = Bot.new(formatted_params)

      result = @bot_validator.call(bot, params[:user])

      if result.success?
        saved_bot = @bots_repository.save(bot)
        Result::Success.new(saved_bot)
      else
        Result::Failure.new(*result.errors)
      end
    end
  end
end
