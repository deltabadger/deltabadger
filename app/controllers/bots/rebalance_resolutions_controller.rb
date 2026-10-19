# Clears a halted (:ambiguous) rebalance so the bot can trade again.
#
# This is a USER-ATTESTED clear, not a proof. The app cannot verify that no order is open: an
# ambiguous halt means a placement came back without a usable order id, and every order read we have
# (Bot::ExchangeUser#get_order, Bot::FetchAndUpdateOpenOrdersJob) needs an id we never stored. So we
# refresh what we CAN see, refuse if a known rebalance order is still live, and otherwise ask the
# user to confirm they checked the venue themselves. Exchange-wide recent-order discovery would let
# us do better; it does not exist yet.
class Bots::RebalanceResolutionsController < ApplicationController
  before_action :authenticate_user!
  before_action :set_bot

  def create
    if (open_order = live_rebalance_order)
      redirect_back fallback_location: bot_path(@bot),
                    alert: t('bot.settings.rebalance.still_open', order_id: open_order.external_id)
      return
    end

    @bot.resolve_rebalance_halt!
    @bot.log_activity('rebalance_manually_resolved', level: :info, details: { user_id: current_user.id })
    redirect_back fallback_location: bot_path(@bot), notice: t('bot.settings.rebalance.resolved')
  end

  private

  def set_bot
    @bot = current_user.bots.find(params[:bot_id])
  end

  # Every rebalance order we could possibly know about — not just the two ids in the pending payload.
  # A worker can die after persist_accepted_order! but before the id reaches the state, leaving a
  # waiting row that the payload never references; missing it here would let the user clear the halt
  # while an order is still live, which is exactly how a duplicate sell happens.
  #
  # Refreshed first so a fill that landed since the halt is seen before we clear on top of it.
  def live_rebalance_order
    pending = @bot.rebalance_pending || {}
    ids = [pending[:sell_transaction_id], pending[:buy_transaction_id]].compact

    candidates = @bot.transactions.rebalance.waiting.or(@bot.transactions.where(id: ids)).distinct
    candidates.find do |transaction|
      Bot::FetchAndUpdateOrderJob.perform_now(transaction, update_missed_quote_amount: false)
      transaction.reload.waiting?
    end
  end
end
