# Lifts a signal bot's unresolved-placement marker (Bots::Signal#unresolved_placement), so it can
# place orders again.
#
# A USER-ATTESTED clear, not a proof, as with Bots::RebalanceResolutionsController: the ambiguous
# placement left no order id to look anything up by, so the user checks the venue and says so.
# Nothing is written to the ledger either way; the marker holds no order data.
#
# The form names the marker it was rendered for. A stale tab, or a second click, finds a different
# marker or none and changes nothing.
class Bots::SignalResolutionsController < ApplicationController
  before_action :authenticate_user!

  def create
    bot = current_user.bots.where(type: 'Bots::Signal').find(params[:bot_id])
    if bot.resolve_unresolved_placement!(params[:marker_id].to_s, user_id: current_user.id)
      redirect_back fallback_location: bot_path(bot), notice: t('bot.signal.resolved')
    else
      redirect_back fallback_location: bot_path(bot), alert: t('bot.signal.resolution_stale')
    end
  end
end
