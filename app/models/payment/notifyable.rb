module Payment::Notifyable
  extend ActiveSupport::Concern

  def send_invoice
    SubscriptionMailer.with(
      payment: self
    ).invoice.deliver_later
  end

  def notify_subscription_granted
    SubscriptionMailer.with(
      payment: self
    ).subscription_granted.deliver_later
  end

  def notify_subscription_granted_manually
    SubscriptionMailer.with(
      payment: self
    ).after_wire_transfer.deliver_later
  end

  def send_wire_transfer_summary
    SubscriptionMailer.with(
      payment: self
    ).wire_transfer_summary.deliver_later
  end
end
