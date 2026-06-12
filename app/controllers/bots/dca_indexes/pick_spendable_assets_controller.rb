class Bots::DcaIndexes::PickSpendableAssetsController < Bots::Wizard::PickSpendableAssetsController
  before_action :require_market_data_configured

  private

  def bot_relation = current_user.bots.dca_index
  def add_api_key_path = new_bots_dca_indexes_add_api_key_path
  def wizard_default_settings = Bots::WizardDefaults::INDEX
  def paginate_asset_list? = false

  def prepare_step
    super
    # Per-currency preview of the chosen index's top base assets, keyed by
    # quote_asset_id. The view renders this as a `.ticker-group` cell in
    # place of the generic exchange-icons cell — same affordance as the
    # ticker pills shown on the index tiles in step 2.
    @top_base_assets_by_quote = @bot.top_base_assets_by_quote_for_current_setup
  end

  def require_market_data_configured
    return if MarketData.configured?

    redirect_to new_bots_dca_indexes_setup_coingecko_path
  end

  def bot_params
    params.require(:bots_dca_index).permit(:quote_asset_id)
  end
end
