module Affiliate::Notifyable
  extend ActiveSupport::Concern

  def send_registration_reminder(amount)
    AffiliateMailer.with(
      referrer: self,
      amount: amount
    ).registration_reminder.deliver_later
  end
end
