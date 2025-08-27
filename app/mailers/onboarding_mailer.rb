class OnboardingMailer < CaffeinateMailer
  has_history
  track_open campaign: -> { "onboarding__#{@mailing.mailer_action}" }
  track_clicks campaign: -> { "onboarding__#{@mailing.mailer_action}" }
  utm_params utm_medium: 'email', utm_source: 'onboarding', utm_campaign: -> { @mailing.mailer_action }

  default from: 'Deltabadger <hello@deltabadger.com>',
          'List-Unsubscribe' => -> { "<#{Caffeinate::UrlHelpers.caffeinate_unsubscribe_url(@mailing.subscription)}>" },
          'List-Unsubscribe-Post' => 'List-Unsubscribe=One-Click'

  layout 'mailers/marketing'

  def fee_cutter(mailing)
    base_mail(mailing)
  end

  def avoid_taxes(mailing)
    base_mail(mailing)
  end

  def referral(mailing)
    @mailing = mailing
    @user = mailing.subscriber
    @content_key = @mailing.mailer_action
    ref_code_path = Rails.application.routes.url_helpers.ref_code_path(code: @user.affiliate.code, locale: nil)
    @ref_link = ENV.fetch('HOME_PAGE_URL') + ref_code_path
    set_locale(@user)

    mail(to: @user.email, subject: "🔑 #{t("onboarding_mailer.#{@content_key}.subject")}")
  end

  def rsi(mailing)
    base_mail(mailing)
  end

  def bitcoin_m2(mailing)
    base_mail(mailing)
  end

  def grayscale_etf(mailing)
    base_mail(mailing)
  end

  def stablecoins(mailing)
    base_mail(mailing)
  end

  def polymarket(mailing)
    base_mail(mailing)
  end

  private

  def base_mail(mailing)
    @mailing = mailing
    @user = mailing.subscriber
    @content_key = @mailing.mailer_action
    set_locale(@user)

    mail(to: @user.email, subject: "🔑 #{t("onboarding_mailer.#{@content_key}.subject")}") do |format|
      format.html { render 'onboarding_mailer/base' }
    end
  end
end
