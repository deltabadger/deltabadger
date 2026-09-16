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
#
# The intent covers a placement with no row to point at. It is NOT the whole of "which sales are
# unaccounted for" — an accepted order the venue later stops reporting is abandoned by
# Bot::StaleOrderResolver, which takes it out of `waiting` without ever saying whether it filled.
# Those ROWS are the other half, and they are the source of truth for themselves: a sale that closes
# several positions at once leaves several outstanding, and one slot holding one name cannot survive
# a sweep, a promotion and a clear without dropping one of them. So the rows block on their own
# account, and a resolution attests about the exact ids it was shown.
module Bot::LiquidationState
  extend ActiveSupport::Concern

  PENDING_KEY = 'liquidation_pending'.freeze
  RESOLVED_KEY = 'liquidation_resolved_orders'.freeze
  SELLING_KEY = 'liquidation_selling_since'.freeze

  STATE_PLACING = 'placing'.freeze
  STATE_AMBIGUOUS = 'ambiguous'.freeze

  # How long a marker nobody cleared is believed. The batch itself is bounded by the exchange
  # semaphore's lease, so this only has to outlive a queue wait.
  SELLING_TTL = 15.minutes

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

  # "Do not trade this bot's assets from any other leg." Intent, an order still working, or one the
  # venue stopped reporting that nobody has accounted for: all three mean coins may be spoken for.
  def liquidation_in_flight?
    liquidation_pending? || transactions.liquidation.waiting.exists? || unresolved_liquidation_orders.exists?
  end

  # LIQUIDATION rows the venue has stopped reporting and the user has not accounted for yet.
  #
  # `abandoned` is OUR conclusion — see Bot::StaleOrderResolver — and on a non-authoritative venue it
  # is not proof the order never executed. It also takes the row out of `waiting`, so before this the
  # row stopped blocking anything the moment it was abandoned. These are the source of truth for
  # "which sales are unaccounted for": a batch leaves several orders outstanding at once, and a
  # single intent slot carrying one name could not survive a sweep, a promotion and a clear without
  # losing one of them.
  #
  # Tracked by id rather than by flipping the row: an attestation is about what the USER checked at
  # the venue, and it must never rewrite what the ledger says happened.
  def unresolved_liquidation_orders
    scope = transactions.liquidation.where(external_status: :abandoned)
    ids = resolved_liquidation_order_ids
    ids.any? ? scope.where.not(id: ids) : scope
  end

  def resolved_liquidation_order_ids
    Array(transient_data[RESOLVED_KEY]).map(&:to_i)
  end

  # Everything the user has to account for before this bot trades again: a placement whose outcome we
  # never learned (no row to point at) and orders the venue stopped reporting (rows that left
  # `waiting` without ever saying whether they filled). One question covers both, and it names all of
  # them.
  def liquidation_halted?
    liquidation_ambiguous? || unresolved_liquidation_orders.exists?
  end

  # The halt as ONE snapshot: the names to show, the order ids an attestation would cover, and the
  # intent nonce if there is one to clear. Read together on purpose — asked as separate queries they
  # can disagree, and a row abandoned between them would be submitted by the Clear button without
  # ever having been named by the warning above it.
  def liquidation_halt
    orders = unresolved_liquidation_orders.to_a
    intent = liquidation_ambiguous? ? liquidation_pending : nil
    { bases: ([intent&.dig(:symbol)] + orders.map(&:base)).compact_blank.uniq,
      order_ids: orders.map(&:id),
      intent_id: intent&.dig(:id) }
  end

  def halted_liquidation_bases = liquidation_halt[:bases]

  # Marks exactly the orders the user was shown, and answers with the rows it actually accounted
  # for. Anything abandoned since that render is NOT in the list and goes on blocking — which is why
  # this needs no generation nonce of its own: attesting about what you were shown cannot wipe
  # uncertainty you were not.
  def resolve_liquidation_orders!(ids)
    rows = unresolved_liquidation_orders.where(id: ids).to_a
    return [] if rows.empty?

    merge_transient_data!(RESOLVED_KEY => resolved_liquidation_order_ids | rows.map(&:id))
    rows
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

  # --- "a sale is under way", for the page only ---------------------------------------------
  #
  # NOT a trading guard. liquidation_blocked_reason never reads any of this, so a marker whose job
  # died before clearing it cannot wedge the bot — the worst it does is show a spinner where a Sell
  # button belongs, for at most SELLING_TTL.
  #
  # It is exactly the lifetime of the request plus its job: written in the REQUEST, which is what
  # covers the unbounded wait on the exchange semaphore, and cleared in the job's ensure. Bounded by
  # SELLING_TTL so a worker killed before its ensure cannot leave it up forever.
  #
  # Deliberately NOT liquidation_in_flight?, even though that is the thing actually refusing a
  # second Sell. Every extra state it counts can outlive the sale, and each one is recovered by
  # pressing Sell again — which is precisely what a spinner would be hiding:
  #
  #   placing intent  — indistinguishable from a worker that died before promoting it, and only a
  #                     further Sell promotes it to the halt that offers Clear
  #   waiting rows    — Bot::FetchAndUpdateOrderJob is one-shot, and on a stopped bot nothing
  #                     re-polls; a further Sell is refused but sweeps them on its way through
  #   ambiguous/abandoned — these make the bot HALTED, which is not progress: it draws its own
  #                     Clear, and the caller gates on may_sell so a halt never reads as selling
  #
  # So the page believes only the marker. The worst it costs is a Sell offered in the seconds after
  # a batch while its orders settle — which is what the page did before any of this, and clicking it
  # is how those orders get swept.
  def liquidation_selling?
    current = transient_data[SELLING_KEY]
    # An epoch Integer rather than a timestamp String: nothing to parse, so a garbage value reads as
    # 0 — expired — instead of raising in a view.
    current.is_a?(Hash) && current['at'].to_i > SELLING_TTL.ago.to_i
  end

  # Returns the token the caller has to hand back to clear it.
  def mark_selling!
    SecureRandom.hex(6).tap do |token|
      merge_transient_data!(SELLING_KEY => { 'id' => token, 'at' => Time.current.to_i })
    end
  end

  # Compare-and-clear under ONE lock, against the row the comparison read.
  #
  # Checking the token on this instance and then calling merge_transient_data! would be two separate
  # reads: a second request writing its own token in between would have that token deleted by our
  # unconditional write, dropping the spinner while its sale is still queued. So the owner check and
  # the delete share the lock, and a stale owner clears nothing.
  def clear_selling!(token)
    return false if token.blank?

    remaining = nil
    self.class.transaction do
      fresh = self.class.lock.find(id)
      current = fresh.transient_data[SELLING_KEY]
      if current.is_a?(Hash) && current['id'] == token
        remaining = fresh.transient_data.except(SELLING_KEY)
        fresh.update_columns(transient_data: remaining)
      end
    end
    return false if remaining.nil?

    write_attribute(:transient_data, remaining)
    clear_attribute_change(:transient_data)
    true
  end

  # The tables only, unlike broadcast_liquidation_state: no holding moves here and the chart is the
  # expensive half, which matters when one of the two callers is a web request. Same rescue as its
  # neighbour — a failed repaint must not fail the request that started the sale, nor mask a job's
  # own exception from the ensure block that calls it.
  #
  # cached_only is for the REQUEST caller. broadcast_metrics_panel renders
  # metrics_with_current_prices, which on a cold five-minute cache goes and asks the exchange — and
  # blocking a request that has just queued a trade, to draw a spinner, is the wrong trade. The page
  # that submitted has rendered this same panel moments ago, so the cache is warm exactly when
  # somebody is watching; a cold one skips, and the job's own repaint still lands. A worker has no
  # such deadline and fetches if it must.
  def broadcast_selling_state(cached_only: false)
    return unless respond_to?(:broadcast_metrics_panel)
    return if cached_only && metrics_with_current_prices_from_cache.nil?

    broadcast_metrics_panel
  rescue StandardError => e
    Rails.logger.warn("selling broadcast failed bot=#{id}: #{e.message}")
  end

  private

  # Bot::StaleOrderResolver marks an order the venue has stopped reporting as `abandoned` after 14
  # days. On a non-authoritative venue that is NOT proof it never executed — and once the row leaves
  # `waiting`, nothing stops a fresh Sell placing on top of a fill we never recorded. So an abandoned
  # liquidation becomes the same halt an unknown placement does.
  #
  # Scoped to rows this sweep just transitioned, so the halt is raised once rather than every tick
  # forever after the user has resolved it.
  #
  # EVERY abandoned row, not just the first. A batch leaves several orders outstanding at once, and
  # a halt naming one of them would be lifted by an attestation the user only gave about that one —
  # freeing the bot to trade while the others are still unaccounted for. Naming them all is what
  # makes the single Clear an honest question.
  # Says so, and repaints. It does NOT write an intent: the abandoned ROWS are what block trading
  # now, so there is nothing here to lose track of — no name to merge into a slot that holds one, and
  # no interaction with a `placing` intent whose placement may still be in flight.
  def halt_abandoned_liquidations!(watched_ids)
    return if watched_ids.empty?

    bases = transactions.liquidation.where(id: watched_ids, external_status: :abandoned).map(&:base).uniq
    return if bases.empty?

    log_activity('liquidation_ambiguous', level: :error,
                                          details: { base: bases.join(', '),
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
