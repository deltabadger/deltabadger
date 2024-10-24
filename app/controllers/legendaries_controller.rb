class LegendariesController < ApplicationController
  before_action :authenticate_user!

  def show
    @subscription = current_user.subscription
    @address_pattern = Ethereum.address_pattern
  end

  def update
    @subscription = current_user.subscription

    if legendary_params[:eth_address_confirmation] && @subscription.update(eth_address: legendary_params[:eth_address])
      ClaimNftMailer.form_submission_email(
        current_user.email,
        current_user.subscription.nft_id,
        current_user.subscription.eth_address
      ).deliver_later
      redirect_to legendary_badger_path
    else
      render :show, status: :unprocessable_entity
    end
  end

  private

  def legendary_params
    params.require(:subscription).permit(:eth_address, :eth_address_confirmation)
  end
end
