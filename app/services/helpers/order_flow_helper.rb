module Helpers
  class OrderFlowHelper
    def initialize(
      notifications: Notifications::BotAlerts.new
    )
      @notifications = notifications
    end

    def stop_bot(bot, notify, errors = ['Something went wrong!'])
      bot.update(status: 'stopped')
      @notifications.error_occured(bot: bot, errors: errors) if notify
    end
  end
end
