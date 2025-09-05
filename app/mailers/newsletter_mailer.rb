class NewsletterMailer < CaffeinateMailer
  has_history
  track_open campaign: -> { "newsletter__#{@mailing.mailer_action}" }
  track_clicks campaign: -> { "newsletter__#{@mailing.mailer_action}" }
  utm_params utm_medium: 'email', utm_source: 'newsletter', utm_campaign: -> { @mailing.mailer_action }

  default from: 'Deltabadger Research <research@deltabadger.com>'

  layout 'mailers/marketing'

  def first_email(mailing)
    base_mail(mailing)
  end

  private

  def base_mail(mailing)
    @mailing = mailing
    @user = mailing.subscriber
    set_locale(@user)

    mail(to: @user.email, subject: "✦ #{t('onetime_campaign_mailer.subject')}") do |format|
      format.html { render 'newsletter_mailer/base' }
    end
  end
end
