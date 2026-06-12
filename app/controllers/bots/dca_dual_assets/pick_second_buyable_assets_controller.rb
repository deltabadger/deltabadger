class Bots::DcaDualAssets::PickSecondBuyableAssetsController < ApplicationController
  before_action :authenticate_user!
  before_action :redirect_if_session_expired, only: :create

  include Bots::Searchable
  include Bots::StockBrokerRoutable

  def new
    @bot = current_user.bots.dca_dual_asset.new(sanitized_bot_config)

    if @bot.base0_asset_id.blank?
      redirect_to new_bots_dca_single_assets_pick_buyable_asset_path
    else
      prepare_step
      nil if render_asset_page(bot: @bot, asset_field: :base1_asset_id)
    end
  end

  def demote_to_single
    cfg = session[:bot_config] || {}
    settings = cfg['settings'] || {}
    base = settings['base0_asset_id'] || settings['base_asset_id']
    session[:bot_config] = {
      'label' => Bots::DcaSingleAsset.new.label,
      'exchange_id' => cfg['exchange_id'],
      'settings' => { 'base_asset_id' => base }.compact
    }.compact
    redirect_to new_bots_dca_single_assets_pick_exchange_path
  end

  def create
    if bot_params[:base1_asset_id].present?
      bot = current_user.bots.dca_dual_asset.new(sanitized_bot_config)
      session[:bot_config].deep_merge!({ settings: bot.parse_params(bot_params) }.deep_stringify_keys)

      base0 = Asset.find_by(id: session.dig(:bot_config, 'settings', 'base0_asset_id'))
      base1 = Asset.find_by(id: bot_params[:base1_asset_id])
      if base0&.category == 'Stock' || base1&.category == 'Stock'
        redirect_after_stock_asset(
          current_user.bots.dca_dual_asset.new(sanitized_bot_config),
          picker_path: new_bots_dca_dual_assets_pick_stock_broker_path,
          add_api_key_path: new_bots_dca_dual_assets_add_api_key_path,
          repick_path: new_bots_dca_dual_assets_pick_second_buyable_asset_path
        )
      else
        redirect_to new_bots_dca_dual_assets_pick_exchange_path
      end
    else
      prepare_step
      render :new, status: :unprocessable_entity
    end
  end

  private

  # View state the :new template needs — shared by `new` and `create`'s 422 re-render.
  def prepare_step
    @bot = current_user.bots.dca_dual_asset.new(sanitized_bot_config)
    @bot.base1_asset_id = nil
    @assets = asset_search_results(@bot, search_params[:query], :base_asset)
  end

  def redirect_if_session_expired
    render turbo_stream: turbo_stream_redirect(root_path) if session[:bot_config].blank?
  end

  def search_params
    params.permit(:query)
  end

  def bot_params
    params.require(:bots_dca_dual_asset).permit(:base1_asset_id)
  end
end
