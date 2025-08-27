class NewsletterMailer < CaffeinateMailer
  include Caffeinate::Helpers

  has_history
  track_open campaign: -> { "newsletter__#{@mailing.mailer_action}" }
  track_clicks campaign: -> { "newsletter__#{@mailing.mailer_action}" }
  utm_params utm_medium: 'email', utm_source: 'newsletter', utm_campaign: -> { @mailing.mailer_action }

  default from: 'Deltabadger Research <research@deltabadger.com>',
          'List-Unsubscribe' => -> { "<#{caffeinate_unsubscribe_url}>" },
          'List-Unsubscribe-Post' => 'List-Unsubscribe=One-Click'

  layout 'mailers/marketing'

  # def first_email(mailing)
  #   base_mail(mailing)
  # end

  private

  def base_mail(mailing)
    @mailing = mailing
    @user = mailing.subscriber
    @content_key = @mailing.mailer_action
    set_locale(@user)

    mail(to: @user.email, subject: "✦ #{t("newsletter_mailer.#{@content_key}.subject")}") do |format|
      format.html { render 'newsletter_mailer/base' }
    end
  end
end
