class Bots::DcaIndexes::AddApiKeysController < Bots::Wizard::AddApiKeysController
  before_action :require_market_data_configured

  private

  def bot_relation = current_user.bots.dca_index
  def missing_exchange_path = new_bots_dca_indexes_pick_exchange_path
  def after_api_key_path = new_bots_dca_indexes_pick_spendable_asset_path

  def require_market_data_configured
    return if MarketData.configured?

    redirect_to new_bots_dca_indexes_setup_coingecko_path
  end
end
