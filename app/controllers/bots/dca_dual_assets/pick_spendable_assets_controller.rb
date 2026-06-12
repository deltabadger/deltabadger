class Bots::DcaDualAssets::PickSpendableAssetsController < Bots::Wizard::PickSpendableAssetsController
  private

  def bot_relation = current_user.bots.dca_dual_asset
  def add_api_key_path = new_bots_dca_dual_assets_add_api_key_path
  def wizard_default_settings = Bots::WizardDefaults::DUAL

  def bot_params
    params.require(:bots_dca_dual_asset).permit(:quote_asset_id)
  end
end
