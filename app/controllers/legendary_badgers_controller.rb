class LegendaryBadgersController < ApplicationController
  before_action :authenticate_user!

  def show
    @subscription = current_user.subscription
  end

  def update
    if legendary_badger_params[:eth_address_confirmation] && current_user.subscription.update(eth_address: legendary_badger_params[:eth_address])
      ClaimNftMailer.form_submission_email(
        current_user.email,
        current_user.subscription.nft_id,
        current_user.subscription.eth_address
      ).deliver_later
      redirect_to legendary_badger_path, notice: I18n.t('legendary_badger.update_success', eth_address: current_user.subscription.eth_address)
    else
      current_user.subscription.reload
      @subscription = current_user.subscription
      # flash.now[:alert] = I18n.t('legendary_badger.invalid_address', eth_address: legendary_badger_params[:eth_address])
      render :show, status: :unprocessable_entity
    end
  end

  private

  def legendary_badger_params
    params.require(:subscription).permit(:eth_address, :eth_address_confirmation)
  end
end