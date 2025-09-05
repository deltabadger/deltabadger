class Drippers::Newsletter < Dripper
  self.campaign = :newsletter
  default mailer: 'NewsletterMailer'

  # WARNING: after adding a new drip step, you need to call rake caffeinate_add_drip_step
  # WARNING: after renaming a drip step, you need to call rake caffeinate_rename_drip

  on_resubscribe do |subscription|
    user = subscription.subscriber
    user.update(subscribed_to_email_marketing: true)
  end

  before_drip do |_drip, mailing|
    user = mailing.subscription.subscriber
    unless user.subscribed_to_email_marketing?
      mailing.subscription.unsubscribe!('Not subscribed to email marketing')
      throw(:abort)
    end
  end

  # Comment the sent drips instead of deleting them, so drip names are not repeated and stats are not
  # mixed with other drips in the campaign
  # drip :first_email, delay: 0.days
end
