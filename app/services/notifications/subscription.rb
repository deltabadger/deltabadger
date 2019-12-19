module Notifications
  class Subscription
    def unlimited_granted(user:)
      SubscriptionMailer
        .with(user: user)
        .unlimited_granted.deliver_later
    end
  end
end
