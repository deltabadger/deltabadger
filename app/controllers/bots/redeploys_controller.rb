# The "Redeploy $243?" prompt's two answers.
#
# Yes puts the idle liquidation proceeds back into the composition at target weights; No takes that
# money off the table for good, so a later sale is offered on its own rather than re-offering what
# the user has already turned down.
class Bots::RedeploysController < ApplicationController
  before_action :authenticate_user!
  before_action :set_bot

  def create
    # A pre-check so a stock composition is told now rather than finding out minutes later in a
    # worker that will not retry. The job checks again — the market can close between enqueue and
    # run — and fails OPEN on any other error, because a convenience check must never be the thing
    # that stops work the job could have done.
    if market_closed?
      flash.now[:alert] = t('bot.redeploy.market_closed')
      return render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end

    Bot::RedeployJob.perform_later(@bot)
    @bot.log_activity('redeploy_requested', level: :info, details: { user_id: current_user.id })
    flash.now[:notice] = t('bot.redeploy.started')
    render turbo_stream: turbo_stream_prepend_flash
  end

  # Queued, not applied here — Bot::DeclineRedeployJob holds the exchange semaphore, which is the
  # only thing that stops a decline interleaving with a batch the worker has already sized. The
  # offset it writes is a snapshot of a figure that batch is still moving; taken mid-flight it
  # strands above the banked total and silently eats every later sale. The job also refuses on its
  # own if a batch is genuinely in flight, and says so in the activity log.
  def destroy
    Bot::DeclineRedeployJob.perform_later(@bot, user_id: current_user.id)
    flash.now[:notice] = t('bot.redeploy.declined')
    render turbo_stream: turbo_stream_prepend_flash
  end

  private

  # Nested bot routes accept every bot type; only composition bots have an offer to answer.
  def set_bot
    @bot = current_user.bots.find(params[:bot_id])
    return if @bot.respond_to?(:redeploy!)

    redirect_back fallback_location: bots_path, alert: t('bot.redeploy.unsupported')
  end

  # Authenticated first: Alpaca answers this from /v2/clock, which needs credentials, and with a
  # cold clock cache an unauthenticated call 401s and raises — turning the button into a 500 before
  # the job that WOULD have authenticated is ever enqueued.
  def market_closed?
    @bot.ensure_exchange_authenticated
    !@bot.exchange.market_open?(tickers: @bot.composition_tickers)
  rescue StandardError => e
    Rails.logger.warn("redeploy market check failed bot=#{@bot.id}: #{e.message}")
    false
  end
end
