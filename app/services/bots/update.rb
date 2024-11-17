module Bots
  class Update < BaseService
    def initialize(
      bots_repository: BotsRepository.new
    )
      @bots_repository = bots_repository
    end

    def call(user, bot_params)
      bot = @bots_repository.by_id_for_user(user, bot_params[:id])
      bot_validator = get_validator(bot)
      format_params = get_formatter(bot)

      settings_params =
        format_params
        .call(bot, bot_params.merge(user: user))
        .slice(:settings)

      if configuration_changed?(bot, settings_params[:settings])
        settings_params = settings_params.merge(settings_changed_at: Time.now)
      end

      bot.assign_attributes(settings_params)
      validation = bot_validator.call(bot, user)

      if validation.success?
        updated_bot = @bots_repository.save(bot)
        Result::Success.new(updated_bot)
      else
        Result::Failure.new(*validation.errors)
      end
    end

    private

    def configuration_changed?(bot, new_settings)
      bot.settings != new_settings
    end

    def get_validator(bot)
      if bot.trading?
        Bots::Trading::Validators::Update.new
      elsif bot.withdrawal?
        Bots::Withdrawal::Validators::Update.new
      else
        Bots::Webhook::Validators::Update.new
      end
    end

    def get_formatter(bot)
      if bot.trading?
        Bots::Trading::FormatParams::Update.new
      elsif bot.withdrawal?
        Bots::Withdrawal::FormatParams::Update.new
      else
        Bots::Webhook::FormatParams::Update.new
      end
    end
  end
end
