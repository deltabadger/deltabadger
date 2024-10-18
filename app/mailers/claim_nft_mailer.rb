class ClaimNftMailer < ApplicationMailer
  default from: 'no-reply@deltabadger.com'

  def form_submission_email(user_email, nft_id, eth_address)
    @user_email = user_email
    @nft_id = nft_id
    @eth_address = eth_address
    mail(to: ['guillem@deltabadger.com', 'jan@deltabadger.com'], subject: 'New NFT Claim Request')
  end
end
