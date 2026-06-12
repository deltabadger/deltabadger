class Bots::DcaDualAssets::PickExchangesController < Bots::Wizard::PickExchangesController
  private

  def bot_relation = current_user.bots.dca_dual_asset
  def add_api_key_path = new_bots_dca_dual_assets_add_api_key_path

  def prerequisite_redirect_path
    new_bots_dca_dual_assets_pick_second_buyable_asset_path if @bot.base1_asset_id.blank?
  end

  def bot_params
    params.require(:bots_dca_dual_asset).permit(:exchange_id)
  end
end
