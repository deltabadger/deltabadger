class Bots::DcaSingleAssets::PickBuyableAssetsController < ApplicationController
  before_action :authenticate_user!

  include Bots::Searchable

  def new
    # Idempotent: do not mutate session here. Turbo prefetches GET requests on
    # hover, so any state mutation here would wipe wizard state from a link hover
    # and cause the wizard to loop back to step 1.
    session[:bot_config] ||= {}
    session[:bot_config]['label'] ||= Bots::DcaSingleAsset.new.label
    @bot = current_user.bots.dca_single_asset.new(sanitized_bot_config)
    # Clear any previously-picked base in memory so the list isn't filtered against it —
    # coming back to step 1 should show the full set, including the current pick.
    @bot.base_asset_id = nil
    @assets = asset_search_results(@bot, search_params[:query], :base_asset)
    nil if render_asset_page(bot: @bot, asset_field: :base_asset_id)
  end

  def create
    if bot_params[:base_asset_id].present?
      # Re-picking the first asset means restarting the wizard: drop any
      # downstream state (exchange, quote, second asset) before storing the new pick.
      label = session.dig(:bot_config, 'label') || Bots::DcaSingleAsset.new.label
      session[:bot_config] = { 'label' => label }
      bot = current_user.bots.dca_single_asset.new(sanitized_bot_config)
      session[:bot_config].deep_merge!({ settings: bot.parse_params(bot_params) }.deep_stringify_keys)

      asset = Asset.find_by(id: bot_params[:base_asset_id])
      if asset&.category == 'Stock'
        set_stock_defaults
        redirect_to new_bots_dca_single_assets_add_api_key_path
      else
        redirect_to new_bots_dca_single_assets_pick_exchange_path
      end
    else
      render :new, status: :unprocessable_entity
    end
  end

  private

  def set_stock_defaults
    alpaca = Exchanges::Alpaca.first
    usd = Asset.find_by(external_id: 'usd')
    session[:bot_config]['exchange_id'] = alpaca.id if alpaca
    session[:bot_config].deep_merge!({ 'settings' => { 'quote_asset_id' => usd.id } }) if usd
  end

  def search_params
    params.permit(:query)
  end

  def bot_params
    params.require(:bots_dca_single_asset).permit(:base_asset_id)
  end
end
