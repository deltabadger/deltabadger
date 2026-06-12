class Bots::DcaDualAssets::PickSecondBuyableAssetsController < Bots::Wizard::PickBuyableAssetsController
  before_action :redirect_if_session_expired, only: :create

  include Bots::StockBrokerRoutable

  def new
    @bot = build_bot

    if @bot.base0_asset_id.blank?
      redirect_to new_bots_dca_single_assets_pick_buyable_asset_path
    else
      prepare_step
      render_asset_page(bot: @bot, asset_field: :base1_asset_id)
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

  private

  def bot_relation = current_user.bots.dca_dual_asset
  def asset_id_param = :base1_asset_id

  def clear_picked_asset(bot)
    bot.base1_asset_id = nil
  end

  def redirect_after_asset_picked
    base0 = Asset.find_by(id: session.dig(:bot_config, 'settings', 'base0_asset_id'))
    base1 = Asset.find_by(id: bot_params[:base1_asset_id])
    if base0&.category == 'Stock' || base1&.category == 'Stock'
      redirect_after_stock_asset(
        build_bot,
        picker_path: new_bots_dca_dual_assets_pick_stock_broker_path,
        add_api_key_path: new_bots_dca_dual_assets_add_api_key_path,
        repick_path: new_bots_dca_dual_assets_pick_second_buyable_asset_path
      )
    else
      redirect_to new_bots_dca_dual_assets_pick_exchange_path
    end
  end

  def bot_params
    params.require(:bots_dca_dual_asset).permit(:base1_asset_id)
  end
end
