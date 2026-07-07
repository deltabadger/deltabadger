class Bots::DcaDualAssets::AddApiKeysController < Bots::Wizard::AddApiKeysController
  include Bots::Wizard::Navigable

  private

  def current_step = :api
  def bot_relation = current_user.bots.dca_dual_asset

  # :currencies (base0) is on the SINGLE picker route; :exchange resolves to the
  # stock-aware missing_exchange_path so a stock bot with no broker bounces to
  # the broker picker.
  def step_path(key)
    case key
    when :currencies  then new_bots_dca_single_assets_pick_buyable_asset_path
    when :currencies2 then new_bots_dca_dual_assets_pick_second_buyable_asset_path
    when :exchange    then missing_exchange_path
    when :api         then new_bots_dca_dual_assets_add_api_key_path
    when :spendable   then new_bots_dca_dual_assets_pick_spendable_asset_path
    end
  end

  def stock_bot?
    @bot.base0_asset&.category == 'Stock' || @bot.base1_asset&.category == 'Stock'
  end

  # A stock bot with no broker yet belongs in the stock-broker step, not the crypto exchange
  # step (reachable via a stale/direct URL before a broker has been chosen).
  def missing_exchange_path
    if stock_bot?
      new_bots_dca_dual_assets_pick_stock_broker_path
    else
      new_bots_dca_dual_assets_pick_exchange_path
    end
  end

  # After (re-)validating the key, go to the first step still missing input
  # (skips an already-chosen asset after an exchange re-pick).
  def after_api_key_path = step_path(first_incomplete)
end
