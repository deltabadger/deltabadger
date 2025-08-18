class ClaimNftMailer < ApplicationMailer
  default from: 'no-reply@deltabadger.com'

  def form_submission_email
    @user_email = params[:user].email
    @nft_id = params[:subscription].nft_id
    @eth_address = params[:subscription].eth_address

    mail(to: ['guillem@deltabadger.com', 'jan@deltabadger.com'], subject: 'New NFT Claim Request')
  end
end
