class Bots::Signals::PickBuyableAssetsController < Bots::Wizard::PickBuyableAssetsController
  def new
    session[:bot_config] ||= {}
    prepare_step
    session[:bot_config]['label'] ||= @bot.label
    render_asset_page(bot: @bot, asset_field: :base_asset_id)
  end

  private

  def bot_relation = current_user.bots.signal
  def asset_id_param = :base_asset_id

  def clear_picked_asset(bot)
    bot.base_asset_id = nil
    bot.exchange_id = nil
  end

  # First wizard step: a POST with an expired session is self-sufficient,
  # mirroring the single-asset first step — re-initialise rather than bail.
  def prepare_session_for_pick
    session[:bot_config] ||= {}
  end

  def redirect_after_asset_picked
    redirect_to new_bots_signals_pick_exchange_path
  end

  def bot_params
    params.require(:bots_signal).permit(:base_asset_id)
  end
end
