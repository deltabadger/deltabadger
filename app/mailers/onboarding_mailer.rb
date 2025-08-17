class OnboardingMailer < CaffeinateMailer
  def welcome_to_my_cool_app(mailing)
    puts 'welcome_to_my_cool_app called'
    @mailing = mailing
    @user = mailing.subscriber

    mail(to: @user.email, subject: 'Welcome to CoolApp!')
  end

  def some_cool_tips(mailing)
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
