class Bots::DcaMultiAssets::PickStockBrokersController < Bots::Wizard::PickStockBrokersController
  include Bots::DcaMultiAssets::WizardSteps

  private

  def bot_params
    params.require(:bots_dca_multi_asset).permit(:exchange_id)
  end
end
