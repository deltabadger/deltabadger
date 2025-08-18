class OnboardingMailer < CaffeinateMailer
  has_history
  track_clicks campaign: -> { "onboarding__#{@mailing.mailer_action}" }
  utm_params utm_medium: 'email', utm_source: 'onboarding', utm_campaign: -> { @mailing.mailer_action }

  def welcome_to_my_cool_app(mailing)
    @mailing = mailing
    @user = mailing.subscriber

    mail(to: @user.email, subject: 'Welcome to CoolApp!')
  end

  def some_cool_tips(mailing)
    @mailing = mailing
    @user = mailing.subscriber

    mail(to: @user.email, subject: 'Here are some cool tips for MyCoolApp')
  end

  def more_cool_tips(mailing)
    @mailing = mailing
    @user = mailing.subscriber

    mail(to: @user.email, subject: 'Here are some cool tips for MyCoolApp')
  end

  def help_getting_started(mailing)
    @mailing = mailing
    @user = mailing.subscriber

    mail(to: @user.email, subject: 'Do you need help getting started?')
  end
end
