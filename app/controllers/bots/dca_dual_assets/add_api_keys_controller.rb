class Bots::DcaDualAssets::AddApiKeysController < Bots::Wizard::AddApiKeysController
  private

  def bot_relation = current_user.bots.dca_dual_asset

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

  # Stock bots get their quote asset auto-filled when the broker is chosen, so
  # pick_spendable_asset only needs the spend amount.
  def after_api_key_path = new_bots_dca_dual_assets_pick_spendable_asset_path

  def after_correct_api_key(api_key)
    sync_alpaca_settings(api_key) if @bot.exchange.is_a?(Exchanges::Alpaca)
  end
end
