class ProductUpdatesMailer < CaffeinateMailer
  has_history
  track_open campaign: -> { "product_updates__#{@mailing.mailer_action}" }
  track_clicks campaign: -> { "product_updates__#{@mailing.mailer_action}" }
  utm_params utm_medium: 'email', utm_source: 'product_updates', utm_campaign: -> { @mailing.mailer_action }

  default from: 'Deltabadger <hello@deltabadger.com>',
          'List-Unsubscribe' => -> { "<#{Caffeinate::UrlHelpers.caffeinate_unsubscribe_url(@mailing.subscription)}>" },
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

    mail(to: @user.email, subject: "✦ #{t("product_updates_mailer.#{@content_key}.subject")}") do |format|
      format.html { render 'product_updates_mailer/base' }
    end
  end
end
