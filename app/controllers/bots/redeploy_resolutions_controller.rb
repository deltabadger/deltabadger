# Clears a halted (ambiguous) redeploy so the bot can trade again.
#
# A USER-ATTESTED clear, not a proof — the same reasoning as Bots::LiquidationResolutionsController.
# An ambiguous halt means a placement came back without a usable order id, and every order read we
# have needs an id we never stored. A balance snapshot cannot separate a filled buy from a resting
# one, and even a confirmed fill yields no price, so there is nothing honest to write to the ledger.
# So we ask the one question the user can answer by looking at the venue.
class Bots::RedeployResolutionsController < ApplicationController
  before_action :authenticate_user!
  before_action :set_bot

  def create
    if (open_order = live_redeploy_order)
      redirect_back fallback_location: bot_path(@bot),
                    alert: t('bot.redeploy.still_open', order_id: open_order.external_id)
      return
    end

    Bot::ResolveRedeployJob.perform_later(@bot, intent_id: params[:intent_id].to_s, user_id: current_user.id)
    redirect_back fallback_location: bot_path(@bot), notice: t('bot.redeploy.resolved')
  end

  private

  def set_bot
    @bot = current_user.bots.find(params[:bot_id])
    return if @bot.respond_to?(:redeploy!)

    redirect_back fallback_location: bots_path, alert: t('bot.redeploy.unsupported')
  end

  # Refreshed first, so a fill that landed since the halt is seen before we clear on top of it.
  def live_redeploy_order
    @bot.transactions.redeploy.waiting.find do |transaction|
      Bot::FetchAndUpdateOrderJob.perform_now(transaction, update_missed_quote_amount: false)
      transaction.reload.waiting?
    end
  end
end
