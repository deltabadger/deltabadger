class AffiliateMailer < ApplicationMailer
  def new_btc_address_confirmation
    @user = params[:user]
    @new_btc_address = params[:new_btc_address]
    @token = params[:token]

    mail(to: @user.email, subject: 'Confirm new Bitcoin address 📒')
  end

  def referrals_payout_notification
    @user = params[:user]
    @amount = params[:amount]

    mail(to: @user.email, subject: "It's a payday! 💸")
  end
end
