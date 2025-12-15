class ProductUpdatesMailer < CaffeinateMailer
  has_history
  track_open campaign: -> { "product_updates__#{@mailing.mailer_action}" }
  track_clicks campaign: -> { "product_updates__#{@mailing.mailer_action}" }
  utm_params utm_medium: 'email', utm_source: 'product_updates', utm_campaign: -> { @mailing.mailer_action }

  default from: 'Deltabadger <hello@deltabadger.com>'

  layout 'mailers/marketing'

  def fireheads_restart(mailing)
    base_mail(mailing)
  end

  def bot_goes_opensource(mailing)
    base_mail(mailing)
  end

  private

  def base_mail(mailing)
    @mailing = mailing
    @user = mailing.subscriber
    set_locale(@user)

    mail(to: @user.email, subject: "#{t('onetime_campaign_mailer.subject')}") do |format|
      format.html { render 'product_updates_mailer/base' }
    end
  end
end
