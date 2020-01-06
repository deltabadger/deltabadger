module Notifications
  class Subscription
    def unlimited_granted(user:)
      SubscriptionMailer
        .with(user: user)
        .unlimited_granted
        .deliver_later
    end

    def invoice(payment:)
      SubscriptionMailer
        .with(user: payment.user, payment: payment)
        .invoice
        .deliver_later
    end
  end
end
