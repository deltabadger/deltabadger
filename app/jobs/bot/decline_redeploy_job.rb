# Takes the offered proceeds off the table, at the user's request.
#
# A job rather than a line in the controller specifically so it holds Bot::ActionJob's semaphore —
# the same reason Bot::ResolveLiquidationJob is one.
#
# The offset it writes is a snapshot of `banked - spent`, and `spent` keeps growing while a batch
# fills. Guarding on the in-flight state from a request narrowed the window but did not close it:
# between the worker sizing its orders and writing its placing intent there is a moment with no
# pending state and no waiting row, where a decline succeeds and the worker places anyway — against
# a figure the user has just declined, and leaving the offset stranded above the banked total where
# it would silently eat every later sale. Sharing the semaphore means the two can never interleave.
class Bot::DeclineRedeployJob < BotJob
  limits_concurrency to: 1,
                     key: ->(bot, *, **) { "exchange_#{bot.exchange&.name_id}" },
                     group: 'Bot::ActionJob'

  def perform(bot, user_id: nil)
    return unless bot.respond_to?(:decline_redeploy!)

    result = bot.decline_redeploy!
    if result.failure?
      # The user has already been told it was taken off the table, so a refusal has to say otherwise
      # somewhere they can find it.
      return bot.log_activity('redeploy_decline_refused', level: :info, details: { reason: result.errors })
    end

    bot.log_activity('redeploy_declined', level: :info, details: { user_id: user_id })
    # Nothing else repaints the panel on its own — the prompt has to disappear.
    bot.broadcast_redeploy_state
  end
end
