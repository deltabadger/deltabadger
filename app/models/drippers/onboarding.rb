class Drippers::Onboarding < Dripper
  self.campaign = :onboarding
  default mailer: 'OnboardingMailer'

  # WARNING: after adding a new drip step, you need to call rake caffeinate_add_drip_step
  # WARNING: after renaming a drip step, you need to call rake caffeinate_rename_drip

  on_unsubscribe do |subscription|
    user = subscription.subscriber
    user.update(subscribed_to_email_marketing: false)
  end

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

  # TODO: adjust delays
  drip :fee_cutter, delay: 0.hours
  drip :avoid_taxes, delay: 1.minutes
  drip :referral, delay: 2.minutes
  drip :rsi, delay: 3.minutes
  drip :bitcoin_m2, delay: 4.minutes
  drip :grayscale_etf, delay: 5.minutes
  drip :stablecoins, delay: 6.minutes
  drip :polymarket, delay: 7.minutes
end
