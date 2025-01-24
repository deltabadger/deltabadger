module Helpers
  class OrderFlowHelper
    def initialize(
      notifications: Notifications::BotAlerts.new,
      bots_repository: BotsRepository.new
    )
      @notifications = notifications
      @bots_repository = bots_repository
    end

    def stop_bot(bot, notify, errors = ['Something went wrong!'])
      bot = @bots_repository.update(bot.id, status: 'stopped')
      @notifications.error_occured(bot: bot, errors: errors) if notify
    end
  end
end
