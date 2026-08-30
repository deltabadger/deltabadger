class Bots::DcaMultiAssets::PickExchangesController < Bots::Wizard::PickExchangesController
  include Bots::DcaMultiAssets::WizardSteps

  private

  def current_step = :exchange
  def prepare_session_for_exchange_pick = reset_downstream!

  def bot_params
    params.require(:bots_dca_multi_asset).permit(:exchange_id)
  end
end
