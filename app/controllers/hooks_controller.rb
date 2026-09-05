# frozen_string_literal: true

# The address a signal bot's rule is called at: POST /hook/<token>. The token is the whole
# authentication — TradingView can send neither a header nor a signature — so this is the one
# unauthenticated path in the app that moves money, and it does as little as it can: one lookup,
# one claim, one enqueue. The body is never read; the trade is defined by the rule, the URL is only
# the trigger. TradingView drops a webhook that takes over three seconds, which is the other reason
# the order is placed by Bot::SignalJob and not here.
#
# ActionController::API for the same reasons as CspReportsController: no session, no CSRF token to
# check (a webhook carries none), no locale switch, no setup redirect, no database read before the
# token lookup. POST only — a GET is a link preview, a prefetch or a curious click.
class HooksController < ActionController::API
  def create
    signal = BotSignal.find_by(token: request.path_parameters[:token])
    return head :not_found if signal.nil? || signal.bot.deleted?

    previous = signal.last_triggered_at
    return ignored('cooldown') unless signal.claim_trigger!

    reason = signal.ignore_reason
    return ignored(reason) if reason

    if enqueue(signal)
      render json: { status: 'accepted' }, status: :accepted
    else
      # The queue is its own database, so the claim and the enqueue can never be one transaction.
      # Hand the claim back or the sender's immediate retry would be swallowed as a cooldown replay
      # of a call that was never kept.
      signal.release_trigger!(previous:)
      head :service_unavailable
    end
  end

  private

  # Acknowledged, not refused: a sender that treats non-2xx as a failed delivery (TradingView does)
  # must not see a call we chose to drop reported as a failure on its side.
  def ignored(reason)
    render json: { status: 'ignored', reason: }, status: :ok
  end

  # perform_later answers false when the adapter refuses quietly, and raises when Solid Queue's own
  # EnqueueError fires — that one is a StandardError, not an ActiveJob::EnqueueError, so Active Job
  # does not fold it into the false. Both mean the same thing here.
  def enqueue(signal)
    Bot::SignalJob.perform_later(signal.bot, signal, signal.last_triggered_at) ? true : false
  rescue SolidQueue::Job::EnqueueError => e
    Rails.logger.error("HooksController: could not enqueue signal #{signal.id}: #{e.message}")
    false
  end
end
