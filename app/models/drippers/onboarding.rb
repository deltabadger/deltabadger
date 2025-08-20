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

  drip :onboarding, delay: 0.hours, options: { content_key: 'fee_cutter' }
  drip :onboarding, delay: 1.day, options: { content_key: 'avoid_taxes' }
  drip :onboarding_referral, delay: 2.days
  drip :onboarding, delay: 4.days, options: { content_key: 'rsi' }
  drip :onboarding, delay: 7.days, options: { content_key: 'bitcoin_m2' }
  drip :onboarding, delay: 14.days, options: { content_key: 'grayscale_etf' }
  drip :onboarding, delay: 21.days, options: { content_key: 'stablecoins' }
  drip :onboarding, delay: 30.days, options: { content_key: 'polymarket' }
end
