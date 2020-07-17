module Notifications
  class Subscription
    def subscription_granted(user:, subscription_plan:)
      SubscriptionMailer
        .with(user: user, subscription_plan: subscription_plan)
        .subscription_granted
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
