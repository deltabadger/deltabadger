class Bots::DcaSingleAssets::PickExchangesController < Bots::Wizard::PickExchangesController
  include Bots::Wizard::Navigable

  def promote_to_multi
    cfg = session[:bot_config] || {}
    settings = cfg['settings'] || {}
    base = settings['base_asset_id'] || Array(settings['base_asset_ids']).first
    session[:bot_config] = {
      'flow' => cfg['flow'],
      'exchange_id' => cfg['exchange_id'],
      'settings' => { 'base_asset_ids' => [base].compact.map(&:to_i) }
    }.compact
    redirect_to new_bots_dca_multi_assets_pick_assets_path
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
