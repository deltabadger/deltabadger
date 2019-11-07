module Bots
  class Update < BaseService
    def initialize(
      bot_validator: Bots::Free::Validator.new,
      format_params: Bots::Free::FormatParams::Update.new,
      bots_repository: BotsRepository.new
    )
      @bots_repository = bots_repository
      @bot_validator = bot_validator
      @format_params = format_params
    end

    def call(user, bot_params)
      bot = @bots_repository.by_id_for_user(user, bot_params[:id])

      settings_params =
        @format_params
        .call(bot, bot_params.merge(user: user))
        .slice(:settings)

      bot.assign_attributes(settings_params)
      validation = @bot_validator.call(bot)

      if validation.success?
        updated_bot = @bots_repository.save(bot)
        Result::Success.new(updated_bot)
      else
        Result::Failure.new(*validation.errors)
      end
    end
  end
end
