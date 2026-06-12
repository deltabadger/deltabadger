class Bots::Signals::PickSpendableAssetsController < Bots::Wizard::PickSpendableAssetsController
  private

  def bot_relation = current_user.bots.signal
  def add_api_key_path = new_bots_signals_add_api_key_path

  # The signals wizard continues to confirm_settings instead of finalising here.
  def after_quote_asset_picked
    redirect_to new_bots_signals_confirm_settings_path
  end

  def bot_params
    params.require(:bots_signal).permit(:quote_asset_id)
  end
end
