class Bots::Signals::AddApiKeysController < Bots::Wizard::AddApiKeysController
  private

  def bot_relation = current_user.bots.signal
  def missing_exchange_path = new_bots_signals_pick_exchange_path
  def after_api_key_path = new_bots_signals_pick_spendable_asset_path
end
