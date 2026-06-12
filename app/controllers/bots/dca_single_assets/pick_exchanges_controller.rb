class Bots::DcaSingleAssets::PickExchangesController < Bots::Wizard::PickExchangesController
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

  def bot_relation = current_user.bots.dca_single_asset
  def add_api_key_path = new_bots_dca_single_assets_add_api_key_path

  def prerequisite_redirect_path
    new_bots_dca_single_assets_pick_buyable_asset_path if @bot.base_asset_id.blank?
  end

  def bot_params
    params.require(:bots_dca_single_asset).permit(:exchange_id)
  end
end
