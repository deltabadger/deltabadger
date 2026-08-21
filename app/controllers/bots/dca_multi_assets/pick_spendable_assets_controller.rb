class Bots::DcaMultiAssets::PickSpendableAssetsController < Bots::Wizard::PickSpendableAssetsController
  include Bots::DcaMultiAssets::WizardSteps

  private

  def current_step = :spendable

  def bot_params
    params.require(:bots_dca_multi_asset).permit(:quote_asset_id)
  end
end
