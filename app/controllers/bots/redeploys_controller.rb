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

  # The decline can be REFUSED — its offset is a snapshot of a figure a running batch is still
  # moving, so taking one mid-flight would strand the offset above the total and silently eat the
  # next sale. The prompt is hidden in that state, but a stale tab or a double submit arrives anyway.
  def destroy
    result = @bot.decline_redeploy!
    if result.failure?
      flash.now[:alert] = t('bot.redeploy.in_flight')
      return render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end

    @bot.broadcast_metrics_update if @bot.respond_to?(:broadcast_metrics_update)
    head :no_content
  end

  private

  # Nested bot routes accept every bot type; only composition bots have an offer to answer.
  def set_bot
    @bot = current_user.bots.find(params[:bot_id])
    return if @bot.respond_to?(:redeploy!)

    redirect_back fallback_location: bots_path, alert: t('bot.redeploy.unsupported')
  end
end
