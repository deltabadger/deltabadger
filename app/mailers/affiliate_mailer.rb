class AffiliateMailer < ApplicationMailer
  def new_btc_address_confirmation
    @user = params[:user]
    @new_btc_address = params[:new_btc_address]
    @token = params[:token]

    mail(to: @user.email, subject: default_i18n_subject)
  end

  def referrals_payout_notification
    @user = params[:user]
    @amount = params[:amount]

    mail(to: @user.email, subject: default_i18n_subject)
  end

  def registration_reminder
    @referrer = params[:referrer]

    mail(to: @referrer.mail, subject: default_i18n_subject)
  end
end
