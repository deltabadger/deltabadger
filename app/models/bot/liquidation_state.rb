# Durable intent for a liquidation placement, and the halt it becomes when the outcome is unknown.
#
# Why a one-leg action needs durable state at all. A rebalance needs it because a sell OWES a buy;
# a liquidation owes nothing, so the obvious argument is that a retry is self-limiting — size at
# `min(ledger amount, free balance)` and an order that already landed has removed those coins from
# free balance. That argument is WRONG whenever the user holds coins of the same asset outside the
# bot: free balance stays positive, the ledger still shows the full position because the fill was
# never recorded, and the retry sells coins that were never the bot's.
#
# Two states, and the difference matters:
#
#   placing    — a network call may be in flight RIGHT NOW. Blocks work; must not be resolvable, or
#                a user could clear a halt out from under a placement that is about to raise one.
#   ambiguous  — the placement's outcome is unknown and definitely not still running. The only state
#                the halt UI and the resolution accept.
#
# A worker that dies mid-placement leaves `placing` with no rescue to promote it, which would be
# invisible and would refuse forever. So any entry that holds the exchange semaphore promotes a stale
# `placing` — holding the lock is itself the proof that no placement of this bot's is running.
#
# The `id` is a generation nonce. Resolution submits the id it rendered and clears only on a match,
# so a stale tab or a double click cannot clear a halt raised by a LATER event, wiping an attestation
# the user never gave for it.
module Bot::LiquidationState
  extend ActiveSupport::Concern

  PENDING_KEY = 'liquidation_pending'.freeze

  STATE_PLACING = 'placing'.freeze
  STATE_AMBIGUOUS = 'ambiguous'.freeze

  def liquidation_pending
    value = transient_data[PENDING_KEY]
    value.is_a?(Hash) ? value.symbolize_keys : nil
  end

  def liquidation_pending?
    liquidation_pending.present?
  end

  def liquidation_ambiguous?
    liquidation_pending&.dig(:state) == STATE_AMBIGUOUS
  end

  # "Do not trade this bot's assets from any other leg." Intent OR an order still working: both mean
  # coins are spoken for.
  def liquidation_in_flight?
    liquidation_pending? || transactions.liquidation.waiting.exists?
  end

  def start_liquidation_placement!(symbol)
    write_liquidation_pending!(id: SecureRandom.hex(6), symbol: symbol, state: STATE_PLACING)
  end

  def flag_liquidation_ambiguous!
    pending = liquidation_pending
    return if pending.nil?

    write_liquidation_pending!(id: pending[:id], symbol: pending[:symbol], state: STATE_AMBIGUOUS)
  end

  def clear_liquidation_pending!
    merge_transient_data!(PENDING_KEY => nil)
  end

  # Advances this bot's own waiting orders. Bot::FetchAndUpdateOrderJob is one-shot and never
  # reschedules, and a STOPPED bot never runs the DCA tick that would otherwise sweep — so without
  # this a liquidation order seen once as open stays `waiting` forever. That matters twice over:
  # waiting rows are the double-click guard AND they stand the other legs down, so a single
  # unresolved row would wedge the whole bot.
  #
  # update_missed_quote_amount: true because the sweep covers EVERY waiting row, DCA buys included,
  # and recording a buy's fill without drawing down the carry leaves it claiming money already spent.
  # Non-contributions are excluded inside the sweep itself.
  def advance_waiting_orders!
    watched = transactions.liquidation.waiting.pluck(:id)
    # Gated on LIQUIDATION rows, not on waiting rows generally. The sweep itself covers everything
    # waiting in one call, and Bot::LimitOrderable already runs it for a resting DCA order — firing
    # here as well would double every exchange read on a bot that has one, for no gain.
    return if watched.empty?

    Bot::FetchAndUpdateOpenOrdersJob.perform_now(self, update_missed_quote_amount: true)
    halt_abandoned_liquidations!(watched)
  rescue StandardError => e
    Rails.logger.warn("liquidation order refresh failed bot=#{id}: #{e.message}")
  end

  # Everything the other legs have to wait for, asked in a way that can also RESOLVE itself. Called
  # from the DCA tick and from a rebalance, both of which hold the exchange semaphore.
  def liquidation_blocks_trading?
    advance_waiting_orders!
    promote_stale_liquidation_placement!
    liquidation_in_flight?
  end

  # Called under the exchange semaphore, so `placing` here cannot be a placement still running — it
  # is a worker that died before it could rescue. Promote it so the user sees a halt they can act on
  # instead of a bot that silently refuses.
  def promote_stale_liquidation_placement!
    return false unless liquidation_pending&.dig(:state) == STATE_PLACING

    flag_liquidation_ambiguous!
    log_activity('liquidation_ambiguous', level: :error,
                                          details: { reason: 'placement intent survived its worker' })
    broadcast_liquidation_state
    true
  end

  # The widget's Sell/Clear swap is driven by this state, and nothing else necessarily refreshes it:
  # trading is blocked while halted, so the periodic broadcasts that would normally repaint the page
  # do not run. Without this the page keeps offering Sell after a halt, or Clear after a resolution,
  # until a manual reload.
  def broadcast_liquidation_state
    broadcast_metrics_update if respond_to?(:broadcast_metrics_update)
  rescue StandardError => e
    Rails.logger.warn("liquidation broadcast failed bot=#{id}: #{e.message}")
  end

  private

  # Bot::StaleOrderResolver marks an order the venue has stopped reporting as `abandoned` after 14
  # days. On a non-authoritative venue that is NOT proof it never executed — and once the row leaves
  # `waiting`, nothing stops a fresh Sell placing on top of a fill we never recorded. So an abandoned
  # liquidation becomes the same halt an unknown placement does.
  #
  # Scoped to rows this sweep just transitioned, so the halt is raised once rather than every tick
  # forever after the user has resolved it.
  def halt_abandoned_liquidations!(watched_ids)
    return if watched_ids.empty? || liquidation_pending?

    abandoned = transactions.liquidation.where(id: watched_ids, external_status: :abandoned).first
    return if abandoned.nil?

    write_liquidation_pending!(id: SecureRandom.hex(6), symbol: abandoned.base, state: STATE_AMBIGUOUS)
    log_activity('liquidation_ambiguous', level: :error,
                                          details: { base: abandoned.base,
                                                     reason: 'exchange stopped reporting the order' })
    broadcast_liquidation_state
  end

  # merge_transient_data!, matching the rebalance state writes: bookkeeping, not a user edit, so no
  # validations and no dirtied `settings` — and locked, so a concurrent web-request write to another
  # key cannot drop this intent on the floor.
  def write_liquidation_pending!(id:, symbol:, state:)
    payload = { 'id' => id, 'symbol' => symbol, 'state' => state }
    merge_transient_data!(PENDING_KEY => payload)
  end
end
