class Bots::DcaSingleAssets::PickStockBrokersController < Bots::Wizard::PickStockBrokersController
  private

  def bot_relation = current_user.bots.dca_single_asset
  def repick_asset_path = new_bots_dca_single_assets_pick_buyable_asset_path
  def add_api_key_path = new_bots_dca_single_assets_add_api_key_path

  def stock_bot?
    @bot.base_asset&.category == 'Stock'
  end

  def bot_params
    params.require(:bots_dca_single_asset).permit(:exchange_id)
  end
end
