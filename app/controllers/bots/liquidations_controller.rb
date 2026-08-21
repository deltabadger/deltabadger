# Sells the assets an index has dropped, at the user's request.
#
# Deliberately manual: closing one of these positions is a taxable disposal, and folding it into
# rebalancing meant a constituent hovering at the index boundary got sold and re-bought on every
# crossing. See Bots::DcaIndex::Liquidatable.
class Bots::LiquidationsController < ApplicationController
  before_action :authenticate_user!
  before_action :set_bot

  # The confirmation modal. It lists the positions rather than asking about them in the abstract:
  # this is an irreversible market sale and a browser confirm() names nothing it is about to sell.
  #
  # Reads the cached metrics only — a live price sweep does not belong in a request, and the page
  # that offered the button has just rendered these same rows, so the cache is warm. With no cache
  # the modal degrades to the question alone.
  def new
    @exited = @bot.exited_holdings(@bot.metrics_with_current_prices_from_cache || {})
  end

  def create
    if market_closed?
      flash.now[:alert] = t('bot.liquidation.market_closed')
      # Unprocessable on purpose: the modal stays open on a failed submit, so the user reads why
      # and closes it themselves rather than watching it vanish as if the sale had started.
      return render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end

    Bot::LiquidateExitedJob.perform_later(@bot)
    @bot.log_activity('liquidation_requested', level: :info, details: { user_id: current_user.id })
    flash.now[:notice] = t('bot.liquidation.started')
    render turbo_stream: turbo_stream_prepend_flash
  end

  private

  # Nested bot routes accept every bot type; only an index can have assets that left an index.
  def set_bot
    @bot = current_user.bots.find(params[:bot_id])
    redirect_back fallback_location: bots_path, alert: t('bot.liquidation.unsupported') unless @bot.is_a?(Bots::DcaIndex)
  end

  # A pre-check so a stock index tells the user now rather than logging it minutes later. The job
  # checks again — the market can close between enqueue and run.
  #
  # Authenticated first: Alpaca answers this from /v2/clock, which needs credentials, and with a cold
  # clock cache an unauthenticated call 401s and raises — turning the Sell button into a 500 before
  # the job that WOULD have authenticated is ever enqueued. And fail OPEN on any other error: a
  # convenience check must never be the thing that stops a sale the job could have made.
  def market_closed?
    @bot.ensure_exchange_authenticated
    !@bot.exchange.market_open?(tickers: @bot.liquidation_tickers)
  rescue StandardError => e
    Rails.logger.warn("liquidation market check failed bot=#{@bot.id}: #{e.message}")
    false
  end
end
