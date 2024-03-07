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

    def after_wire_transfer(user:, subscription_plan:, name:, type:, amount:)
      SubscriptionMailer
        .with(
          user: user,
          subscription_plan: subscription_plan,
          name: name,
          type: type,
          amount: amount
        )
        .after_wire_transfer
        .deliver_later
    end

    def wire_transfer_summary(
      id:,
      email:,
      subscription_plan:,
      first_name:,
      last_name:,
      country:,
      amount:
    )
      SubscriptionMailer
        .with(
          id: id,
          email: email,
          subscription_plan: subscription_plan,
          first_name: first_name,
          last_name: last_name,
          country: country,
          amount: amount
        )
        .wire_transfer_summary
        .deliver_later
    end
  end
end
