class Bots::DcaDualAssets::PickExchangesController < Bots::Wizard::PickExchangesController
  include Bots::Wizard::Navigable

  private

  def current_step = :exchange
  def bot_relation = current_user.bots.dca_dual_asset

  # :currencies (base0) is always picked on the SINGLE picker route.
  def step_path(key)
    case key
    when :currencies  then new_bots_dca_single_assets_pick_buyable_asset_path
    when :currencies2 then new_bots_dca_dual_assets_pick_second_buyable_asset_path
    when :exchange    then new_bots_dca_dual_assets_pick_exchange_path
    when :api         then new_bots_dca_dual_assets_add_api_key_path
    when :spendable   then new_bots_dca_dual_assets_pick_spendable_asset_path
    end
  end

  # exchange → api is invariant across both variants.
  def add_api_key_path = new_bots_dca_dual_assets_add_api_key_path

  # Re-picking the exchange keeps the chosen assets and drops the quote.
  def prepare_session_for_exchange_pick = reset_downstream!

  def bot_params
    params.require(:bots_dca_dual_asset).permit(:exchange_id)
  end
end
