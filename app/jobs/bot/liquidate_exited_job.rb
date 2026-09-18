# Sells the positions the user named — one, or every row of the quitters table.
#
# NO retry_on. A job-level retry around order placement replays the placement, which is the known
# double-buy bug class: placement has no idempotency key, so a retried "failure" that actually landed
# places a second order — for a batch, N of them rather than one. An unknown outcome halts as
# ambiguous inside Bot::Composition::Liquidatable instead of being replayed.
class Bot::LiquidateExitedJob < BotJob
  # Room for a placement to finish inside the lease, sized on Client's 30s read timeout. A heuristic,
  # not a guarantee: a placement is not always one request — Clients::Ibkr#place_order submits and
  # then loops confirmations — and covering that worst case would need more than the whole three
  # minutes, so a value that guaranteed it would refuse every sale instead.
  #
  # What it is NOT covering is bounded. A placement that overruns leaves `placing` written before the
  # network call, and liquidation_blocked_reason refuses any pending intent — so a second sale still
  # cannot place beside it. The residue is a spurious "unconfirmed" halt on a live placement, which
  # is what the single-sale path has always risked; this only keeps the batch from making it likelier.
  PLACEMENT_HEADROOM = 45.seconds

  # Explicitly joins Bot::ActionJob's semaphore. Solid Queue's concurrency_group defaults to
  # `self.class.name` and the key is [group, param].join("/"), so same-key different-class jobs do
  # NOT share a lock — without this a DCA tick, a rebalance and a liquidation would all run against
  # each other's stale balances. Holding this lock is also what lets Bot::LiquidationState treat a
  # surviving `placing` intent as a dead worker rather than a live placement.
  limits_concurrency to: 1,
                     key: ->(bot, *, **) { "exchange_#{bot.exchange&.name_id}" },
                     group: 'Bot::ActionJob'

  # selling_token defaults to nil so a job serialised before this shipped still deserialises — and a
  # nil token simply clears nothing, leaving its marker to expire on the TTL.
  #
  # holdings: `[[key, asset_id], ...]`, the asset each key named when the sale was requested.
  def perform(bot, holdings: nil, symbols: nil, symbol: nil, selling_token: nil)
    # ponytail: `symbols:` / `symbol:` are the names before holdings carried their asset, kept for sales
    # enqueued in the seconds before this deploy (no asset to check: sold by key as then). These arguments
    # are serialised in solid_queue_jobs, and a keyword mismatch raises BEFORE the rescue below — leaving
    # the user a "sale started" flash and no activity row at all. Delete after one deploy.
    holdings ||= Array(symbols.nil? ? symbol : symbols).map { |key| [key, nil] }
    return unless bot.respond_to?(:liquidate!)
    # Logged, not silent: the controller has already told the user the sale started, so a bot that
    # was archived or disconnected between the click and the run must say why nothing happened
    # rather than leaving a false success standing.
    return refuse(bot, 'archived') if bot.deleted? || bot.archived?
    return refuse(bot, 'api_key_pending') if bot.api_key&.pending_activation?

    bot.ensure_exchange_authenticated
    return unless market_open?(bot, holdings)

    result = bot.liquidate!(holdings: holdings, deadline: batch_deadline)
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
  ensure
    # Every exit takes the spinner down: the two refusals above, the market-closed return, the
    # failure log and the rescue that re-raises. Clearing is owner-checked, so a second request that
    # queued behind us keeps its own marker and its own spinner — and the broadcast only fires when
    # this job's clear actually landed, so a losing clear does not repaint over a live one.
    bot.broadcast_selling_state if bot.respond_to?(:clear_selling!) && bot.clear_selling!(selling_token)
  end

  private

  # The batch may keep STARTING holdings only while the exclusion it runs under still holds, and that
  # exclusion is Solid Queue's semaphore lease: taken when the job is dispatched, never renewed, so
  # queue delay spends it too. A fixed window measured from here would not notice that and would
  # happily run on past it — letting a second sale in between our holdings, where there is no intent
  # and no waiting row to stand it down.
  #
  # nil when there is no lease to read (an inline run), and the model falls back to its own window.
  def batch_deadline
    # concurrency_key is computed from `arguments`, which are empty when perform is called straight
    # on a bare instance — an inline run, which was never dispatched and so holds no lease either.
    return nil if arguments.blank?

    lease = SolidQueue::Semaphore.find_by(key: concurrency_key)&.expires_at
    lease && (lease - PLACEMENT_HEADROOM)
  end

  def refuse(bot, reason)
    bot.log_activity('liquidation_not_started', level: :info, details: { reason: reason })
  end

  # A stock composition must not place into a closed market. Asked about the tickers actually being sold,
  # not the whole catalogue. Logged rather than silently dropped: this is a one-shot user command, so
  # the reason has to land somewhere the user can find it.
  def market_open?(bot, holdings)
    return true if bot.exchange.market_open?(tickers: bot.liquidation_tickers(holdings: holdings))

    bot.log_activity('liquidation_market_closed', level: :info)
    false
  end
end
