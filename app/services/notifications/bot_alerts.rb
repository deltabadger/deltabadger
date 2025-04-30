module Notifications
  class BotAlerts
    RESTART_THRESHOLD = 4

    def initialize(
      calculate_restart_delay: CalculateRestartDelay.new,
      format_readable_duration: FormatReadableDuration.new
    )
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
        restart_at: Time.current + calculate_restart_delay.call(bot.restarts),
        errors: errors
      ).notify_about_restart.deliver_later
    end

    def end_of_funds(bot:)
      return if bot.last_end_of_funds_notification && bot.last_end_of_funds_notification > 1.day.ago

      bot.update(last_end_of_funds_notification: DateTime.now)

      BotAlertsMailer.with(
        bot: bot,
        user: bot.user,
        quote: bot.quote
      ).end_of_funds.deliver_later
    end

    def successful_webhook_bot_transaction(bot:, type:)
      BotAlertsMailer.with(
        bot: bot,
        bot_name: bot.name,
        type: type,
        user: bot.user,
        base: bot.base,
        quote: bot.quote,
        price: bot.price
      ).successful_webhook_bot_transaction.deliver_later
    end

    private

    def was_sent_more_than_day_ago?(notification_sent_at)
      notification_sent_at.nil? || notification_sent_at < (Date.current - 1.days)
    end

    attr_reader :calculate_restart_delay, :format_readable_duration
  end
end
