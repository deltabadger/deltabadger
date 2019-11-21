module Notifications
  class BotAlerts
    def error_occured(bot:, errors: [])
      BotAlertsMailer.with(
        user: bot.user,
        bot: bot,
        errors: errors
      ).notify_about_error.deliver_later
    end

    def limit_reached(bot:)
      BotAlertsMailer.with(
        bot: bot
      ).limit_reached.deliver_later
    end
  end
end
