class AffiliateMailer < ApplicationMailer
  def new_btc_address_confirmation
    @user = params[:user]
    @token = params[:token]

    byebug
    mail(to: @user.email, subject: 'Confirm bitcoin address update')
  end
end
