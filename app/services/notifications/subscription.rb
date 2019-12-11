module Notifications
  class Subscription
    def unlimited_granted(user:)
      return nil if user.subscription.limit_almost_reached_sent

      SubscriptionMailer
        .with(user: user)
        .unlimited_granted.deliver_later
    end
  end
end
