class Bots::DcaSingleAssets::PickBuyableAssetsController < Bots::Wizard::PickBuyableAssetsController
  include Bots::Wizard::Navigable
  include Bots::StockBrokerRoutable

  def new
    # Idempotent: do not mutate session here. Turbo prefetches GET requests on
    # hover, so any state mutation here would wipe wizard state from a link hover
    # and cause the wizard to loop back to step 1.
    session[:bot_config] ||= {}
    session[:bot_config]['label'] ||= Bots::DcaSingleAsset.new.label
    @bot = build_bot

    # Exchange-first reaches this step after exchange+api; bounce back if an
    # upstream pick is missing. In asset-first this is the first step, so the
    # order-derived guard is a no-op.
    if (path = prerequisite_redirect_path)
      redirect_to path
      return
    end

    prepare_step
    render_asset_page(bot: @bot, asset_field: :base_asset_id)
  end

  private

  def current_step = :currencies
  def bot_relation = current_user.bots.dca_single_asset
  def asset_id_param = :base_asset_id

  def step_path(key)
    case key
    when :currencies then new_bots_dca_single_assets_pick_buyable_asset_path
    when :exchange   then new_bots_dca_single_assets_pick_exchange_path
    when :api        then new_bots_dca_single_assets_add_api_key_path
    when :spendable  then new_bots_dca_single_assets_pick_spendable_asset_path
    end
  end

  def clear_picked_asset(bot)
    bot.base_asset_id = nil
  end

  # Re-committing this step clears everything it (and later steps) own — in
  # asset-first that is the whole wizard (the historic "restart" wipe); the
  # label is preserved so the bot keeps its name across the restart.
  def prepare_session_for_pick
    session[:bot_config] ||= {}
    session[:bot_config]['label'] ||= Bots::DcaSingleAsset.new.label
    reset_downstream!
  end

  def redirect_after_asset_picked
    asset = Asset.find_by(id: bot_params[:base_asset_id])
    # Stock routing only applies asset-first (asset picked before the venue). In
    # exchange-first the venue is already chosen, so just advance — routing here
    # would wipe the chosen exchange and bounce to the broker picker.
    if asset_first? && asset&.category == 'Stock'
      redirect_after_stock_asset(
        build_bot,
        picker_path: new_bots_dca_single_assets_pick_stock_broker_path,
        add_api_key_path: new_bots_dca_single_assets_add_api_key_path,
        repick_path: new_bots_dca_single_assets_pick_buyable_asset_path
      )
    else
      advance!
    end
  end

  def bot_params
    params.require(:bots_dca_single_asset).permit(:base_asset_id)
  end
end
