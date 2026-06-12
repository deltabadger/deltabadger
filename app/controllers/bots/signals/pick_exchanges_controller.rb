class Bots::Signals::PickExchangesController < Bots::Wizard::PickExchangesController
  private

  def bot_relation = current_user.bots.signal
  def add_api_key_path = new_bots_signals_add_api_key_path

  def prerequisite_redirect_path
    new_bots_signals_pick_buyable_asset_path if @bot.base_asset_id.blank?
  end

  def bot_params
    params.require(:bots_signal).permit(:exchange_id)
  end
end
