module Notifications
  class BotAlerts
    def error_occured(bot:, user:, errors: [])
      BotAlertsMailer.with(
        user: user,
        bot: bot,
        errors: errors
      ).notify_about_error.deliver_later
    end
  end
end
