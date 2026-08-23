# Puts a liquidation's proceeds back into the composition, at the user's request.
#
# NO retry_on, for the same reason Bot::LiquidateExitedJob has none: a job-level retry around order
# placement replays the placement, and placement has no idempotency key — a retried "failure" that
# actually landed places a second order. An unknown outcome halts as ambiguous inside
# Bot::Composition::Redeployable instead of being replayed.
class Bot::RedeployJob < BotJob
  # Explicitly joins Bot::ActionJob's semaphore. Solid Queue's concurrency_group defaults to
  # `self.class.name` and the key is [group, param].join("/"), so same-key different-class jobs do
  # NOT share a lock — without this a DCA tick, a rebalance, a liquidation and a redeploy would all
  # run against each other's stale balances. Holding this lock is also what lets a surviving
  # `placing` intent be read as a dead worker rather than a live placement.
  limits_concurrency to: 1,
                     key: ->(bot, *, **) { "exchange_#{bot.exchange&.name_id}" },
                     group: 'Bot::ActionJob'

  def perform(bot)
    return unless bot.respond_to?(:redeploy!)
    # Logged, not silent: the controller has already told the user it started, so a bot archived or
    # disconnected between the click and the run must say why nothing happened rather than leaving a
    # false success standing.
    return refuse(bot, 'archived') if bot.deleted? || bot.archived?
    return refuse(bot, 'api_key_pending') if bot.api_key&.pending_activation?

    # Deliberately NOT gated on the bot being started. A composition bot is a portfolio container and
    # the schedule is only one of the ways money reaches it — this leg is a peer of rebalancing,
    # which also runs while the DCA leg is stopped.
    bot.ensure_exchange_authenticated

    result = bot.redeploy!
    return unless result&.failure?

    # Every guard that can decline — a rebalance mid-swap, a standing halt, a composition refresh
    # that failed, an offer that has since gone to zero — has to say so somewhere the user can find
    # it, because they have already been told it started.
    bot.log_activity('redeploy_not_started', level: :info, details: { reason: result.errors })
  rescue StandardError => e
    # Its own event, not redeploy_not_started: that one means a guard declined and its wording says
    # so. An exception did not, and the user had been told it started — so a rejected key, a rate
    # limit or a failed balance read would otherwise leave a flash and no trace, with the reason
    # buried where only the operator can see it.
    #
    # Nothing that reached the venue is written off here: a placement records its intent BEFORE the
    # network call and clears it only in the same transaction as the row, so anything genuinely in
    # flight is still promoted to a halt on the next attempt.
    #
    # Re-raised, so it is still a failed execution for the operator too.
    bot.log_activity('redeploy_failed', level: :error, details: { reason: e.message })
    raise
  end

  private

  def refuse(bot, reason)
    bot.log_activity('redeploy_not_started', level: :info, details: { reason: reason })
  end
end
