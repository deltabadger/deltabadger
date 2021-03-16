module Helpers
  class OrderFlowHelper
    def initialize(
      notifications: Notifications::BotAlerts.new,
      bots_repository: BotsRepository.new,
      validate_limit: Bots::Free::Validators::Limit.new,
      validate_almost_limit: Bots::Free::Validators::AlmostLimit.new,
      validate_trial_ending_soon: Bots::Free::Validators::TrialEndingSoon.new
    )
      @notifications = notifications
      @bots_repository = bots_repository
      @validate_limit = validate_limit
      @validate_almost_limit = validate_almost_limit
      @validate_trial_ending_soon = validate_trial_ending_soon
    end

    def stop_bot(bot, notify, errors = ['Something went wrong!'])
      bot = @bots_repository.update(bot.id, status: 'stopped')
      @notifications.error_occured(bot: bot, errors: errors) if notify
    end

    def check_if_trial_ending_soon(bot, notify)
      ending_soon_result = @validate_trial_ending_soon.call(bot.user)
      @notifications.first_month_ending_soon(bot: bot) if ending_soon_result.failure? && notify
    end

    def validate_limit(bot, notify)
      validate_limit_result = @validate_limit.call(bot.user)
      if validate_limit_result.failure?
        bot = @bots_repository.update(bot.id, status: 'stopped')
        @notifications.limit_reached(bot: bot) if notify
      elsif @validate_almost_limit.call(bot.user).failure? && notify
        @notifications.limit_almost_reached(bot: bot)
      end

      validate_limit_result
    end
  end
end
