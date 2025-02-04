module Notifications
  class Subscription
    def subscription_granted(payment:)
      SubscriptionMailer
        .with(payment: payment)
        .subscription_granted
        .deliver_later(wait: 23.hours)
    end

    def invoice(payment:)
      SubscriptionMailer
        .with(payment: payment)
        .invoice
        .deliver_later
    end

    def after_wire_transfer(payment:)
      SubscriptionMailer
        .with(payment: payment)
        .after_wire_transfer
        .deliver_later
    end

    def wire_transfer_summary(payment:)
      SubscriptionMailer
        .with(payment: payment)
        .wire_transfer_summary
        .deliver_later
    end
  end
end
