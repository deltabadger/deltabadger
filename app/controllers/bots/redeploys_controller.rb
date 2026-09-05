# The "Redeploy $243?" prompt's two answers.
#
# Yes puts the idle liquidation proceeds back into the composition at target weights; No takes that
# money off the table for good, so a later sale is offered on its own rather than re-offering what
# the user has already turned down.
class Bots::RedeploysController < ApplicationController
  before_action :authenticate_user!
  before_action :set_bot

  # One implementation: BotApi::Bots::AnswerRedeploy also backs the MCP tool and the REST
  # endpoint, so the market check and the enqueue cannot drift between them. The service
  # pre-checks the market so a stock composition is told now rather than finding out minutes
  # later in a worker that will not retry; the job checks again, and both fail OPEN on any other
  # error, because a convenience check must never be the thing that stops work the job could do.
  def create
    answer(accept: true, notice: 'bot.redeploy.started')
  end

  # Queued, not applied here — Bot::DeclineRedeployJob holds the exchange semaphore, which is the
  # only thing that stops a decline interleaving with a batch the worker has already sized. The
  # offset it writes is a snapshot of a figure that batch is still moving; taken mid-flight it
  # strands above the banked total and silently eats every later sale. The job also refuses on its
  # own if a batch is genuinely in flight, and says so in the activity log.
  def destroy
    answer(accept: false, notice: 'bot.redeploy.declined')
  end

  private

  def answer(accept:, notice:)
    result = BotApi::Bots::AnswerRedeploy.call(user: current_user, bot_id: @bot.id, accept: accept)
    if result.success?
      flash.now[:notice] = t(notice)
      render turbo_stream: turbo_stream_prepend_flash
    else
      flash.now[:alert] = result.error_code == 'market_closed' ? t('bot.redeploy.market_closed') : result.error_message
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  # Nested bot routes accept every bot type; only composition bots have an offer to answer.
  def set_bot
    @bot = current_user.bots.find(params[:bot_id])
    return if @bot.respond_to?(:redeploy!)

    redirect_back fallback_location: bots_path, alert: t('bot.redeploy.unsupported')
  end
end
