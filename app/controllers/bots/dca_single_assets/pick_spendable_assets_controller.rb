class Bots::DcaSingleAssets::PickSpendableAssetsController < Bots::Wizard::PickSpendableAssetsController
  include Bots::Wizard::Navigable

  private

  def current_step = :spendable
  def bot_relation = current_user.bots.dca_single_asset
  def add_api_key_path = new_bots_dca_single_assets_add_api_key_path
  def wizard_default_settings = Bots::WizardDefaults::SINGLE

  def step_path(key)
    case key
    when :currencies then new_bots_dca_single_assets_pick_buyable_asset_path
    when :exchange   then new_bots_dca_single_assets_pick_exchange_path
    when :api        then new_bots_dca_single_assets_add_api_key_path
    when :spendable  then new_bots_dca_single_assets_pick_spendable_asset_path
    end
  end

  def bot_params
    params.require(:bots_dca_single_asset).permit(:quote_asset_id)
  end
end
