# Sells the positions a user named on a composition bot — one row's Sell, or every row of the
# quitters table via its Sell all.
#
# Deliberately manual: closing one of these positions is a taxable disposal, and folding it into
# rebalancing meant a member hovering at the composition boundary got sold and re-bought on every
# crossing. See Bot::Composition::Liquidatable.
class Bots::LiquidationsController < ApplicationController
  before_action :authenticate_user!
  before_action :set_bot
  before_action :set_symbols

  # The confirmation modal. It names every position rather than asking in the abstract: these are
  # irreversible market sales and a browser confirm() says nothing about what it is closing. That
  # is what answers the old objection to a bulk button.
  #
  # Reads the cached metrics only — a live price sweep does not belong in a request, and the page
  # that offered the button has just rendered these same rows, so the cache is warm. A row whose
  # price is missing still gets listed, with the amount off the price-free ledger: "Sell these
  # positions?" over an empty list names nothing it is about to sell, and a row with no amount at
  # all renders as a flat 0, which is worse than saying nothing.
  def new
    priced = @bot.sellable_holdings(@bot.metrics_with_current_prices_from_cache || {}).index_by { |h| h[:symbol] }
    tickers = @bot.tickers.index_by(&:base)
    ledger = @bot.metrics[:asset_breakdown] || {}
    @holdings = @symbols.map do |symbol|
      priced[symbol] || { symbol: symbol, ticker: tickers[symbol], amount: ledger.dig(symbol, :amount) }
    end
    @exited = (@symbols - @bot.exited_symbols).empty?
  end

  # One implementation: BotApi::Bots::LiquidateExited also backs the MCP tool and the REST
  # endpoint, so the market check, the enqueue and the activity row cannot drift between them.
  def create
    return refuse(t('settings.wash_sale.prompt_missing')) unless wash_sale_answer_recorded?

    result = BotApi::Bots::LiquidateExited.call(user: current_user, bot_id: @bot.id, symbol: @symbols)
    if result.success?
      flash.now[:notice] = t(@symbols.many? ? 'bot.liquidation.started_all' : 'bot.liquidation.started')
      render turbo_stream: turbo_stream_prepend_flash
    else
      # Unprocessable on purpose: the modal stays open on a failed submit, so the user reads why
      # and closes it themselves rather than watching it vanish as if the sale had started.
      refuse(result.error_code == 'market_closed' ? t('bot.liquidation.market_closed') : result.error_message)
    end
  end

  private

  # This sale can be the one that starts a window, so the account's answer is recorded BEFORE the
  # order is enqueued. A modal appended to the response would arrive after the sale was already on
  # its way, which is the one thing the question cannot be late for.
  def wash_sale_answer_recorded?
    return true if current_user.wash_sale_decided?

    record_wash_sale_answer == true
  end

  def refuse(message)
    flash.now[:alert] = message
    render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
  end

  # Nested bot routes accept every bot type; only composition bots hold sellable positions.
  def set_bot
    @bot = current_user.bots.find(params[:bot_id])
    redirect_back fallback_location: bots_path, alert: t('bot.liquidation.unsupported') unless @bot.respond_to?(:held_symbols)
  end

  # The symbols come from the URL, so they are user input: something the bot does not hold must not
  # be reachable by hand-editing it. INTERSECTED with held_symbols rather than checked for
  # membership — a name sold from another tab between the render and the click drops out of the sale
  # instead of refusing the whole click with a bare 404. held_symbols needs no prices, so a cold
  # metrics cache still cannot turn a live Sell button into one.
  #
  # There is deliberately no "no symbols means everything" fallback: a request that names nothing is
  # a bug, and a bug must not be able to mean sell everything.
  def set_symbols
    @symbols = Array(params[:symbol]).map(&:to_s).uniq & @bot.held_symbols
    head :not_found if @symbols.empty?
  end
end
