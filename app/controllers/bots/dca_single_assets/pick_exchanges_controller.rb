class Bots::DcaSingleAssets::PickExchangesController < ApplicationController
  before_action :authenticate_user!
  before_action :redirect_if_session_expired, only: :create

  include Bots::Searchable

  def new
    @bot = current_user.bots.dca_single_asset.new(sanitized_bot_config)

    if @bot.base_asset_id.blank?
      redirect_to new_bots_dca_single_assets_pick_buyable_asset_path
    else
      prepare_step
    end
  end

  def create
    if bot_params[:exchange_id].present?
      session[:bot_config].merge!({ exchange_id: bot_params[:exchange_id] }.stringify_keys)
      redirect_to new_bots_dca_single_assets_add_api_key_path
    else
      prepare_step
      render :new, status: :unprocessable_entity
    end
  end

  def promote_to_dual
    cfg = session[:bot_config] || {}
    settings = cfg['settings'] || {}
    base = settings.delete('base_asset_id') || settings['base0_asset_id']
    session[:bot_config] = {
      'label' => Bots::DcaDualAsset.new.label,
      'exchange_id' => cfg['exchange_id'],
      'settings' => { 'base0_asset_id' => base }.compact
    }.compact
    redirect_to new_bots_dca_dual_assets_pick_second_buyable_asset_path
  end

  private

  # View state the :new template needs — shared by `new` and `create`'s 422 re-render.
  def prepare_step
    @bot = current_user.bots.dca_single_asset.new(sanitized_bot_config)
    @bot.exchange_id = nil
    @exchanges = exchange_search_results(@bot, search_params[:query])
  end

  def redirect_if_session_expired
    render turbo_stream: turbo_stream_redirect(root_path) if session[:bot_config].blank?
  end

  def search_params
    params.permit(:query)
  end

  def bot_params
    params.require(:bots_dca_single_asset).permit(:exchange_id)
  end
end
