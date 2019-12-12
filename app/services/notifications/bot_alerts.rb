module Notifications
  class BotAlerts
    def initialize(subscriptions_repository: SubscriptionsRepository.new)
      @subscriptions_repository = subscriptions_repository
    end

    def error_occured(bot:, errors: [])
      BotAlertsMailer.with(
        user: bot.user,
        bot: bot,
        errors: errors
      ).notify_about_error.deliver_later
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

      @subscriptions_repository.update(subscription.id, limit_almost_reached_sent: true)

      BotAlertsMailer.with(
        bot: bot,
        user: bot.user
      ).limit_almost_reached.deliver_later
    end
  end
end
