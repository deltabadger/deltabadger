class AffiliateMailer < ApplicationMailer
  def new_btc_address_confirmation
    @user = params[:user]
    @new_btc_address = params[:new_btc_address]
    @token = params[:token]

    mail(to: @user.email, subject: 'Confirm bitcoin address update')
  end
end
