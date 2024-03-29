class LegendaryBadgerController < ApplicationController
  before_action :authenticate_user!

  layout 'legendary_badger'

  def show
    @subscription = current_user.subscription
    puts @subscription.inspect
  end

  def update
    if current_user.subscription.update(legendary_badger_params)
      redirect_to legendary_badger_path, notice: 'Address was successfully added.'
    else
      flash.now[:alert] = "#{current_user.subscription.eth_address} is an invalid Ethereum address. Please check your input."
      @subscription = current_user.subscription.reload
      render :show
    end
  end

  private

  def legendary_badger_params
    params.require(:subscription).permit(:eth_address)
  end
end
