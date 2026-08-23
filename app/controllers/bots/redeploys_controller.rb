# The "Redeploy $243?" prompt's two answers.
#
# Yes puts the idle liquidation proceeds back into the composition at target weights; No takes that
# money off the table for good, so a later sale is offered on its own rather than re-offering what
# the user has already turned down.
class Bots::RedeploysController < ApplicationController
  before_action :authenticate_user!
  before_action :set_bot

  def create
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
end
