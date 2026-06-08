class Bots::DcaDualAssets::PickStockBrokersController < ApplicationController
  before_action :authenticate_user!

  include Bots::StockBrokerRoutable

  def new
    @bot = current_user.bots.dca_dual_asset.new(sanitized_bot_config)
    return redirect_to new_bots_dca_dual_assets_pick_second_buyable_asset_path unless stock_bot?

    @brokers = available_stock_brokers(@bot)
    redirect_to new_bots_dca_dual_assets_pick_second_buyable_asset_path if @brokers.empty?
  end

  def create
    @bot = current_user.bots.dca_dual_asset.new(sanitized_bot_config)
    return redirect_to new_bots_dca_dual_assets_pick_second_buyable_asset_path unless stock_bot?

    @brokers = available_stock_brokers(@bot)
    chosen = @brokers.find { |exchange| exchange.id.to_s == bot_params[:exchange_id].to_s }
    if chosen
      finalize_stock_broker!(chosen)
      redirect_to new_bots_dca_dual_assets_add_api_key_path
    else
      render :new, status: :unprocessable_entity
    end
  end

  private

  def stock_bot?
    @bot.base0_asset&.category == 'Stock' || @bot.base1_asset&.category == 'Stock'
  end

  def bot_params
    params.require(:bots_dca_dual_asset).permit(:exchange_id)
  end
end
