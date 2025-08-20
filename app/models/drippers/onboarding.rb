class Drippers::Onboarding < Dripper
  self.campaign = :onboarding
  default mailer: 'OnboardingMailer'

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

  drip :fee_cutter, delay: 0.hours
  drip :avoid_taxes, delay: 1.day
  drip :referral, delay: 2.day
  drip :rsi, delay: 4.days
  drip :bitcoin_m2, delay: 7.days
  drip :grayscale_etf, delay: 14.days
  drip :stablecoins, delay: 21.days
  drip :polymarket, delay: 30.days
end
