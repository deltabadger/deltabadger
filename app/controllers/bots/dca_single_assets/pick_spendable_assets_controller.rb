class Bots::DcaSingleAssets::PickSpendableAssetsController < Bots::Wizard::PickSpendableAssetsController
  private

  def bot_relation = current_user.bots.dca_single_asset
  def add_api_key_path = new_bots_dca_single_assets_add_api_key_path
  def wizard_default_settings = Bots::WizardDefaults::SINGLE

  def bot_params
    params.require(:bots_dca_single_asset).permit(:quote_asset_id)
  end
end
