class Bots::DcaSingleAssets::AddApiKeysController < Bots::Wizard::AddApiKeysController
  include Bots::Wizard::Navigable

  private

  def current_step = :api
  def bot_relation = current_user.bots.dca_single_asset

  # :exchange resolves to the stock-aware missing_exchange_path so a stock bot
  # with no broker bounces to the broker picker rather than the crypto picker.
  def step_path(key)
    case key
    when :currencies then new_bots_dca_single_assets_pick_buyable_asset_path
    when :exchange   then missing_exchange_path
    when :api        then new_bots_dca_single_assets_add_api_key_path
    when :spendable  then new_bots_dca_single_assets_pick_spendable_asset_path
    end
  end

  def stock_bot?
    @bot.base_asset&.category == 'Stock'
  end

  # A stock bot with no broker yet belongs in the stock-broker step, not the crypto exchange
  # step (reachable via a stale/direct URL before a broker has been chosen).
  def missing_exchange_path
    if stock_bot?
      new_bots_dca_single_assets_pick_stock_broker_path
    else
      new_bots_dca_single_assets_pick_exchange_path
    end
  end

  # After (re-)validating the key, go to the first step still missing input. In
  # the linear flow that is the next step (asset-first → spendable, exchange-first
  # → asset); after re-picking the exchange it skips the already-chosen asset and
  # lands on the quote.
  def after_api_key_path = step_path(first_incomplete)

  def after_correct_api_key(api_key)
    sync_alpaca_settings(api_key) if @bot.exchange.is_a?(Exchanges::Alpaca)
  end
end
