# Sells the assets an index has dropped, at the user's request.
#
# NO retry_on. A job-level retry around order placement replays the placement, which is the known
# double-buy bug class: placement has no idempotency key, so a retried "failure" that actually landed
# places a second order. An unknown outcome halts as ambiguous inside Bots::DcaIndex::Liquidatable
# instead of being replayed.
class Bot::LiquidateExitedJob < BotJob
  # Explicitly joins Bot::ActionJob's semaphore. Solid Queue's concurrency_group defaults to
  # `self.class.name` and the key is [group, param].join("/"), so same-key different-class jobs do
  # NOT share a lock — without this a DCA tick, a rebalance and a liquidation would all run against
  # each other's stale balances. Holding this lock is also what lets Bot::LiquidationState treat a
  # surviving `placing` intent as a dead worker rather than a live placement.
  limits_concurrency to: 1,
                     key: ->(bot, *) { "exchange_#{bot.exchange&.name_id}" },
                     group: 'Bot::ActionJob'

  def perform(bot)
    return unless bot.is_a?(Bots::DcaIndex)
    # Logged, not silent: the controller has already told the user the sale started, so a bot that
    # was archived or disconnected between the click and the run must say why nothing happened
    # rather than leaving a false success standing.
    return refuse(bot, 'archived') if bot.deleted? || bot.archived?
    return refuse(bot, 'api_key_pending') if bot.api_key&.pending_activation?

    bot.ensure_exchange_authenticated
    return unless market_open?(bot)

    result = bot.liquidate_exited!
    # A refusal here is silent otherwise, and the user has already been told the sale started. Every
    # guard that can decline — a rebalance mid-swap, a standing halt, a composition refresh that
    # failed — has to say so somewhere the user can find it.
    return unless result&.failure?

    bot.log_activity('liquidation_not_started', level: :info,
                                                details: { reason: result.errors })
  rescue StandardError => e
    # Its own event, not liquidation_not_started: that one means a guard declined and its wording
    # says so. Every guard that DECLINES already reports itself; an exception did not, and the user
    # had been told the sale started — so a rejected key, a rate limit or a failed balance read left
    # a flash and no trace at all, with the reason buried in solid_queue_failed_executions where
    # only the operator can see it. Nothing that reached the venue is being written off here: a placement
    # records its intent BEFORE the network call and only clears it in the same transaction as the
    # row, so anything genuinely in flight is still promoted to a halt on the next attempt.
    #
    # Re-raised, so the failure is still a failed execution for the operator as well.
    bot.log_activity('liquidation_failed', level: :error, details: { reason: e.message })
    raise
  end

  private

  def refuse(bot, reason)
    bot.log_activity('liquidation_not_started', level: :info, details: { reason: reason })
  end

  # A stock index must not place into a closed market. Asked about the tickers actually being sold,
  # not the whole catalogue. Logged rather than silently dropped: this is a one-shot user command, so
  # the reason has to land somewhere the user can find it.
  def market_open?(bot)
    return true if bot.exchange.market_open?(tickers: bot.liquidation_tickers)

    bot.log_activity('liquidation_market_closed', level: :info)
    false
  end
end
