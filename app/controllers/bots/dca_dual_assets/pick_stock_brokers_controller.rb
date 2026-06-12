class Bots::DcaDualAssets::PickStockBrokersController < Bots::Wizard::PickStockBrokersController
  private

  def bot_relation = current_user.bots.dca_dual_asset
  def repick_asset_path = new_bots_dca_dual_assets_pick_second_buyable_asset_path
  def add_api_key_path = new_bots_dca_dual_assets_add_api_key_path

  def stock_bot?
    @bot.base0_asset&.category == 'Stock' || @bot.base1_asset&.category == 'Stock'
  end

  def bot_params
    params.require(:bots_dca_dual_asset).permit(:exchange_id)
  end
end
