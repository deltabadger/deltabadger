class Bots::DcaSingleAssets::PickExchangesController < Bots::Wizard::PickExchangesController
  include Bots::Wizard::Navigable

  # Rewrite the single-asset session into a dual one (base_asset_id → base0) and
  # hand off to the dual second-asset step. The chosen exchange and the flow
  # variant are preserved so the user stays in their chosen order. Always lands
  # on :currencies2 (base0 is set, base1 is not) in either variant.
  def promote_to_dual
    cfg = session[:bot_config] || {}
    settings = cfg['settings'] || {}
    base = settings.delete('base_asset_id') || settings['base0_asset_id']
    session[:bot_config] = {
      'label' => Bots::DcaDualAsset.new.label,
      'flow' => cfg['flow'],
      'exchange_id' => cfg['exchange_id'],
      'settings' => { 'base0_asset_id' => base }.compact
    }.compact
    redirect_to new_bots_dca_dual_assets_pick_second_buyable_asset_path
  end

  private

  def current_step = :exchange
  def bot_relation = current_user.bots.dca_single_asset

  def step_path(key)
    case key
    when :currencies then new_bots_dca_single_assets_pick_buyable_asset_path
    when :exchange   then new_bots_dca_single_assets_pick_exchange_path
    when :api        then new_bots_dca_single_assets_add_api_key_path
    when :spendable  then new_bots_dca_single_assets_pick_spendable_asset_path
    end
  end

  # exchange → api is invariant across both variants.
  def add_api_key_path = new_bots_dca_single_assets_add_api_key_path

  # Re-picking the exchange keeps the chosen asset (the exchange list is
  # asset-filtered, so it stays valid) and drops the pair-specific quote.
  def prepare_session_for_exchange_pick = reset_downstream!

  def bot_params
    params.require(:bots_dca_single_asset).permit(:exchange_id)
  end
end
