class LegendariesController < ApplicationController
  before_action :authenticate_user!

  def show
    @subscription = current_user.subscription
    puts @subscription.inspect
  end

  def update
    if current_user.subscription.update(legendary_params)
      redirect_to legendary_path, notice: I18n.t('legendary.update_success', eth_address: current_user.subscription.eth_address)
    else
      flash.now[:alert] = I18n.t('legendary.invalid_address', eth_address: current_user.subscription.eth_address)
      @subscription = current_user.subscription.reload
      render :show
    end
  end

  private

  def legendary_params
    params.require(:subscription).permit(:eth_address)
  end
end
