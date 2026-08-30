# Clears a halted (ambiguous) liquidation so the bot can trade again.
#
# This is a USER-ATTESTED clear, not a proof. An ambiguous halt means a placement came back without a
# usable order id, and every order read we have needs an id we never stored — Bot::ExchangeUser
# #get_orders takes explicit ids and this codebase has no exchange-wide recent-order discovery. A
# balance snapshot cannot separate a filled order from a resting one either, and even a confirmed
# fill yields no price, so there is nothing honest to write to the ledger. So we ask the one question
# the user can actually answer by looking at the venue.
class Bots::LiquidationResolutionsController < ApplicationController
  before_action :authenticate_user!
  before_action :set_bot

  def create
    if (open_order = live_liquidation_order)
      redirect_back fallback_location: bot_path(@bot),
                    alert: t('bot.liquidation.still_open', order_id: open_order.external_id)
      return
    end

    # Enqueued rather than cleared here: the job holds the exchange semaphore, so it cannot clear an
    # intent while a placement is still in flight. It also re-checks the generation id.
    Bot::ResolveLiquidationJob.perform_later(@bot, intent_id: params[:intent_id].to_s, user_id: current_user.id)
    redirect_back fallback_location: bot_path(@bot), notice: t('bot.liquidation.resolved')
  end

  private

  def set_bot
    @bot = current_user.bots.find(params[:bot_id])
    redirect_back fallback_location: bots_path, alert: t('bot.liquidation.unsupported') unless @bot.respond_to?(:exited_symbols)
  end

  # Refreshed first, so a fill that landed since the halt is seen before we clear on top of it.
  def live_liquidation_order
    @bot.transactions.liquidation.waiting.find do |transaction|
      Bot::FetchAndUpdateOrderJob.perform_now(transaction, update_missed_quote_amount: false)
      transaction.reload.waiting?
    end
  end
end
