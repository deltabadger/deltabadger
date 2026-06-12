class Bots::DcaSingleAssets::PickBuyableAssetsController < Bots::Wizard::PickBuyableAssetsController
  include Bots::StockBrokerRoutable

  def new
    # Idempotent: do not mutate session here. Turbo prefetches GET requests on
    # hover, so any state mutation here would wipe wizard state from a link hover
    # and cause the wizard to loop back to step 1.
    session[:bot_config] ||= {}
    session[:bot_config]['label'] ||= Bots::DcaSingleAsset.new.label
    prepare_step
    render_asset_page(bot: @bot, asset_field: :base_asset_id)
  end

  private

  def bot_relation = current_user.bots.dca_single_asset
  def asset_id_param = :base_asset_id

  def clear_picked_asset(bot)
    bot.base_asset_id = nil
  end

  # Re-picking the first asset means restarting the wizard: drop any
  # downstream state (exchange, quote, second asset) before storing the new pick.
  def prepare_session_for_pick
    label = session.dig(:bot_config, 'label') || Bots::DcaSingleAsset.new.label
    session[:bot_config] = { 'label' => label }
  end

  def redirect_after_asset_picked
    asset = Asset.find_by(id: bot_params[:base_asset_id])
    if asset&.category == 'Stock'
      redirect_after_stock_asset(
        build_bot,
        picker_path: new_bots_dca_single_assets_pick_stock_broker_path,
        add_api_key_path: new_bots_dca_single_assets_add_api_key_path,
        repick_path: new_bots_dca_single_assets_pick_buyable_asset_path
      )
    else
      redirect_to new_bots_dca_single_assets_pick_exchange_path
    end
  end

  def bot_params
    params.require(:bots_dca_single_asset).permit(:base_asset_id)
  end
end
