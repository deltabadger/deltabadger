class OnboardingMailer < CaffeinateMailer
  has_history
  track_open campaign: -> { "onboarding__#{@mailing.mailer_action}" }
  track_clicks campaign: -> { "onboarding__#{@mailing.mailer_action}" }
  utm_params utm_medium: 'email', utm_source: 'onboarding', utm_campaign: -> { @mailing.mailer_action }

  def onboarding(mailing)
    @mailing = mailing
    @user = mailing.subscriber
    @content_key = mailing.drip.options[:content_key] || 'fee_cutter'

    mail(
      to: @user.email, 
      subject: "🔑 #{t("mailer_onboarding.#{@content_key}.subject")}"
    ) do |format|
      format.html { render layout: 'mailer_newsletter' }
    end
  end

  def onboarding_referral(mailing)
    @mailing = mailing
    @user = mailing.subscriber
    @ref_link = ENV.fetch('HOME_PAGE_URL') + Rails.application.routes.url_helpers.ref_code_path(code: @user.affiliate.code, locale: nil)

    mail(
      to: @user.email, 
      subject: "🔑 #{t("mailer_onboarding.referral.subject")}"
    ) do |format|
      format.html { render layout: 'mailer_newsletter' }
    end
  end
end
