class LegendariesController < ApplicationController
  before_action :authenticate_user!
  before_action :redirect_to_upgratde_if_not_legendary
  before_action :set_show_instance_variables, only: %i[show update]

  def show; end

  def update
    if legendary_params[:eth_address_confirmation] && @subscription.update(eth_address: legendary_params[:eth_address])
      ClaimNftMailer.with(
        user: current_user,
        subscription: @subscription
      ).form_submission_email.deliver_later
      redirect_to legendary_path
    else
      render :show, status: :unprocessable_entity
    end
  end

  private

  def set_show_instance_variables
    @subscription = current_user.subscription
    @address_pattern = Ethereum.address_pattern
  end

  def legendary_params
    params.require(:subscription).permit(:eth_address, :eth_address_confirmation)
  end

  def redirect_to_upgratde_if_not_legendary
    redirect_to upgrade_path unless current_user.subscription.legendary?
  end
end
