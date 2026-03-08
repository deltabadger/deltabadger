class Bots::DcaSingleAssets::PickExchangesController < ApplicationController
  before_action :authenticate_user!

  include Bots::Searchable

  def new
    @bot = current_user.bots.dca_single_asset.new(session[:bot_config])

    if @bot.base_asset_id.blank?
      redirect_to new_bots_dca_single_assets_pick_buyable_asset_path
    else
      @bot.exchange_id = nil
      @exchanges = exchange_search_results(@bot, search_params[:query])
    end
  end

  def create
    if bot_params[:exchange_id].present?
      session[:bot_config].merge!({ exchange_id: bot_params[:exchange_id] }.stringify_keys)
      redirect_to new_bots_dca_single_assets_add_api_key_path
    else
      render :new, status: :unprocessable_entity
    end
  end

  private

  def search_params
    params.permit(:query)
  end

  def bot_params
    params.require(:bots_dca_single_asset).permit(:exchange_id)
  end
end
