class Bots::DcaDualAssets::PickFirstBuyableAssetsController < ApplicationController
  before_action :authenticate_user!

  include Bots::Searchable

  def new
    session[:bot_config] = { 'label' => session.dig(:bot_config, 'label') || Bots::DcaDualAsset.new.label }
    @bot = current_user.bots.dca_dual_asset.new(sanitized_bot_config)
    @assets = asset_search_results(@bot, search_params[:query], :base_asset)
    nil if render_asset_page(bot: @bot, asset_field: :base0_asset_id)
  end

  def create
    if bot_params[:base0_asset_id].present?
      bot = current_user.bots.dca_dual_asset.new(sanitized_bot_config)
      session[:bot_config].deep_merge!({ settings: bot.parse_params(bot_params) }.deep_stringify_keys)
      redirect_to new_bots_dca_dual_assets_pick_second_buyable_asset_path
    else
      render :new, status: :unprocessable_entity
    end
  end

  private

  def search_params
    params.permit(:query)
  end

  def bot_params
    params.require(:bots_dca_dual_asset).permit(:base0_asset_id)
  end
end
