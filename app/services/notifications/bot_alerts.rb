module Notifications
  class BotAlerts
    RESTART_THRESHOLD = 4

    def initialize(
      subscriptions_repository: SubscriptionsRepository.new,
      calculate_restart_delay: CalculateRestartDelay.new,
      format_readable_duration: FormatReadableDuration.new
    )
      @subscriptions_repository = subscriptions_repository
      @calculate_restart_delay = calculate_restart_delay
      @format_readable_duration = format_readable_duration
    end

    def error_occured(bot:, errors: [])
      BotAlertsMailer.with(
        user: bot.user,
        bot: bot,
        errors: errors
      ).notify_about_error.deliver_later
    end

    def restart_occured(bot:, errors: [])
      return unless bot.restarts >= RESTART_THRESHOLD

      BotAlertsMailer.with(
        user: bot.user,
        bot: bot,
        delay: format_readable_duration.call(calculate_restart_delay.call(bot.restarts)),
        errors: errors
      ).notify_about_restart.deliver_later
    end

    def limit_reached(bot:)
      BotAlertsMailer.with(
        bot: bot,
        user: bot.user
      ).limit_reached.deliver_later
    end

    def limit_almost_reached(bot:)
      subscription = bot.user.subscription

      return nil if subscription.limit_almost_reached_sent

      subscriptions_repository.update(subscription.id, limit_almost_reached_sent: true)

      BotAlertsMailer.with(
        bot: bot,
        user: bot.user
      ).limit_almost_reached.deliver_later
    end

    def first_month_ending_soon(bot:)
      BotAlertsMailer.with(
        bot: bot,
        user: bot.user
      ).first_month_ending_soon.deliver_later
    end

    private

    attr_reader :subscriptions_repository, :calculate_restart_delay, :format_readable_duration
  end
end
