# Shared stock-broker routing for the bot-creation wizard. Stock bots used to hardcode
# Exchanges::Alpaca.first; now they route through a "pick stock broker" step that lets the
# user choose between the available stock venues (Alpaca / IBKR), auto-skipping the picker
# when only one venue qualifies.
#
# The auto-select decision is made on a POST (the stock asset pick or the broker pick), never
# on a GET — the wizard relies on idempotent GETs because Turbo prefetches them on hover.
module Bots::StockBrokerRoutable
  extend ActiveSupport::Concern

  private

  # Stock venues that list every base asset of the bot, quoted in USD. The USD quote constraint
  # is applied in memory so we don't persist a quote default before the user has actually chosen
  # (or auto-skipped to) a broker. Mirrors the buyable-asset picker, which also lists venues from
  # Exchange.available — so a venue never appears as an asset yet vanishes as a broker.
  def available_stock_brokers(bot)
    bot.quote_asset_id ||= usd_asset&.id
    bot.available_exchanges_for_current_settings.select(&:stock_venue?)
  end

  # Persist the chosen broker + the USD quote default into the wizard session. Mirrors the
  # behaviour of the old set_stock_defaults (exchange_id + USD quote), but with a
  # user-/availability-chosen exchange instead of Exchanges::Alpaca.first.
  def finalize_stock_broker!(exchange)
    session[:bot_config] ||= {}
    session[:bot_config]['exchange_id'] = exchange.id
    return unless (usd = usd_asset)

    session[:bot_config].deep_merge!({ 'settings' => { 'quote_asset_id' => usd.id } })
  end

  # Decide where a freshly-picked stock asset goes:
  #   1 broker  -> finalize it + go straight to the api-key step (preserves zero-click UX)
  #   2+ brokers -> the broker picker
  #   0 brokers  -> back to re-pick the asset (e.g. a mixed stock+crypto pair shares no venue)
  def redirect_after_stock_asset(bot, picker_path:, add_api_key_path:, repick_path:)
    brokers = available_stock_brokers(bot)
    if brokers.one?
      finalize_stock_broker!(brokers.first)
      redirect_to add_api_key_path
    else
      # Drop any broker an earlier single-asset step auto-selected (or the user chose before
      # promoting to dual): with 2+ candidates the choice must be made here, and with 0 (a mixed
      # stock+crypto pair) it is invalid. add_api_key only guards on a blank exchange_id, so a
      # stale selection would otherwise slip through a direct navigation to that step.
      session[:bot_config]&.delete('exchange_id')
      redirect_to brokers.size >= 2 ? picker_path : repick_path
    end
  end

  def usd_asset
    @usd_asset ||= Asset.find_by(external_id: 'usd')
  end
end
