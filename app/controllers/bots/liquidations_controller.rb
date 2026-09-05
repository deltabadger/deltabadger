# Sells an asset a bot's composition has dropped, at the user's request.
#
# Deliberately manual: closing one of these positions is a taxable disposal, and folding it into
# rebalancing meant a member hovering at the composition boundary got sold and re-bought on every
# crossing. See Bot::Composition::Liquidatable.
class Bots::LiquidationsController < ApplicationController
  before_action :authenticate_user!
  before_action :set_bot
  before_action :set_exited_symbol

  # The confirmation modal. It names the position rather than asking in the abstract: this is an
  # irreversible market sale and a browser confirm() says nothing about what it is closing.
  #
  # Reads the cached metrics only — a live price sweep does not belong in a request, and the page
  # that offered the button has just rendered this same row, so the cache is warm. With no cache the
  # modal degrades to the question alone, which is why the symbol is NOT validated against this.
  def new
    @holding = @bot.exited_holdings(@bot.metrics_with_current_prices_from_cache || {})
                   .find { |holding| holding[:symbol] == @symbol }
  end

  # One implementation: BotApi::Bots::LiquidateExited also backs the MCP tool and the REST
  # endpoint, so the market check, the enqueue and the activity row cannot drift between them.
  def create
    result = BotApi::Bots::LiquidateExited.call(user: current_user, bot_id: @bot.id, symbol: @symbol)
    if result.success?
      flash.now[:notice] = t('bot.liquidation.started')
      render turbo_stream: turbo_stream_prepend_flash
    else
      flash.now[:alert] = result.error_code == 'market_closed' ? t('bot.liquidation.market_closed') : result.error_message
      # Unprocessable on purpose: the modal stays open on a failed submit, so the user reads why
      # and closes it themselves rather than watching it vanish as if the sale had started.
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  private

  # Nested bot routes accept every bot type; only composition bots expose exited holdings.
  def set_bot
    @bot = current_user.bots.find(params[:bot_id])
    redirect_back fallback_location: bots_path, alert: t('bot.liquidation.unsupported') unless @bot.respond_to?(:exited_symbols)
  end

  # The symbol comes from the URL, so it is user input: a current member, or something the
  # bot does not hold, must not be reachable by hand-editing it — that would be a taxable disposal
  # the composition never asked for. Checked against exited_symbols, which needs no prices, so a cold
  # metrics cache cannot turn a live Sell button into a 404.
  def set_exited_symbol
    @symbol = params[:symbol].to_s
    head :not_found unless @bot.exited_symbols.include?(@symbol)
  end
end
