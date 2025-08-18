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

  drip :welcome_to_my_cool_app, delay: 0.hours
  drip :some_cool_tips, delay: 2.days
  drip :more_cool_tips, delay: 2.5.days
  drip :help_getting_started, delay: 3.days
end
