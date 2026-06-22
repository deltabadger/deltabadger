class Bots::DcaDualAssets::PickSecondBuyableAssetsController < Bots::Wizard::PickBuyableAssetsController
  before_action :redirect_if_session_expired, only: :create

  include Bots::Wizard::Navigable
  include Bots::StockBrokerRoutable

  def new
    @bot = build_bot

    # base0 is picked on the single route; if it (or any upstream step in
    # exchange-first) is missing, the order-derived guard bounces back there.
    if (path = prerequisite_redirect_path)
      redirect_to path
    else
      prepare_step
      render_asset_page(bot: @bot, asset_field: :base1_asset_id)
    end
  end

  # Rewrite the dual session back into a single one (base0 → base_asset_id),
  # preserving the exchange and flow variant. Redirects to the single flow's last
  # step; its order-derived prerequisite guard pulls back to the first gap
  # (exchange+api+base filled ⇒ stays on :spendable).
  def demote_to_single
    cfg = session[:bot_config] || {}
    settings = cfg['settings'] || {}
    base = settings['base0_asset_id'] || settings['base_asset_id']
    session[:bot_config] = {
      'label' => Bots::DcaSingleAsset.new.label,
      'flow' => cfg['flow'],
      'exchange_id' => cfg['exchange_id'],
      'settings' => { 'base_asset_id' => base }.compact
    }.compact
    redirect_to new_bots_dca_single_assets_pick_spendable_asset_path
  end

  private

  def current_step = :currencies2
  def bot_relation = current_user.bots.dca_dual_asset
  def asset_id_param = :base1_asset_id

  # :currencies (base0) is always picked on the SINGLE picker route.
  def step_path(key)
    case key
    when :currencies  then new_bots_dca_single_assets_pick_buyable_asset_path
    when :currencies2 then new_bots_dca_dual_assets_pick_second_buyable_asset_path
    when :exchange    then new_bots_dca_dual_assets_pick_exchange_path
    when :api         then new_bots_dca_dual_assets_add_api_key_path
    when :spendable   then new_bots_dca_dual_assets_pick_spendable_asset_path
    end
  end

  def clear_picked_asset(bot)
    bot.base1_asset_id = nil
  end

  # Re-committing the second asset clears the picks after it (quote in either
  # variant; exchange too in asset-first), keeping base0.
  def prepare_session_for_pick = reset_downstream!

  def redirect_after_asset_picked
    base0 = Asset.find_by(id: session.dig(:bot_config, 'settings', 'base0_asset_id'))
    base1 = Asset.find_by(id: bot_params[:base1_asset_id])
    # Stock routing only applies asset-first (see the single picker for why).
    if asset_first? && (base0&.category == 'Stock' || base1&.category == 'Stock')
      redirect_after_stock_asset(
        build_bot,
        picker_path: new_bots_dca_dual_assets_pick_stock_broker_path,
        add_api_key_path: new_bots_dca_dual_assets_add_api_key_path,
        repick_path: new_bots_dca_dual_assets_pick_second_buyable_asset_path
      )
    else
      advance!
    end
  end

  def bot_params
    params.require(:bots_dca_dual_asset).permit(:base1_asset_id)
  end
end
