# After a position is sold at a loss, buying it back inside the jurisdiction's window disallows (US,
# Ireland) or matches away (UK) the loss — and the bot is exactly the thing that would buy it back,
# on the very next tick, because a sold constituent is the most underweight name in its composition.
# So the guard lives on the bot: a constituent sold at a loss is locked out of the DCA buy leg, the
# rebalance leg and the redeploy leg until the window has passed, and the composition tables show
# the countdown.
#
# Only the look-FORWARD window is enforced. The Sell button closes the whole position, and when
# every share goes in one order the shares bought inside the window before it are sold too — their
# basis absorbs the disallowed loss and the full loss is recognised; only a repurchase afterwards can
# undo it. A rebalance sell is partial, so its look-back is NOT covered (the manual says so).
#
# Lock arithmetic: the sale day is day 0, days 1..N are inside, buying resumes at the start of day
# N+1. Calendar days in the app's zone, which is what the rules count.
#
# The lock is written by the sale legs in the same transaction as their placement intent and
# cleared only on a provable pre-transmission failure — see Bot::Composition::Liquidatable and
# Bot::Rebalancer.
module Bot::WashSaleGuard
  extend ActiveSupport::Concern

  # The setting lives on the taxpayer (User#wash_sale_days); this concern is the seam every trading
  # leg talks to, so the legs never learn where it is stored.
  delegate :wash_sale_days, :locked_asset_ids, to: :user

  # Buying resumes at the start of the day after the window.
  #
  # ponytail: calendar days in the APP's zone, not the jurisdiction's. There is no persisted tax
  # residence to derive one from, and the countdown and the row render in the app's zone too. The
  # residue is at most a day, and only where the app's zone runs behind the jurisdiction's — a sale
  # seen in that hour would unlock on the window's last day. Per-jurisdiction calendars if that
  # matters; the tax report over the whole account is the authority either way.
  def lock_deadline(from: Time.zone.today)
    (from + wash_sale_days + 1).beginning_of_day
  end

  # Locks the asset through the window that starts on `from`, never pulling an existing deadline in.
  # Returns the CLAIM — what stood before, and the token this placement wrote — so the caller can
  # put the first back if the sale it guards provably never left, and so the rollback can tell its
  # own claim from somebody else's. nil while the rule is off.
  def lock_buying!(asset_id, from: Time.zone.today)
    return if wash_sale_days.zero?

    lock = wash_sale_lock_for(asset_id)
    previous = lock.buy_locked_until
    token = SecureRandom.hex(8)
    lock.update_columns(buy_locked_until: [previous, lock_deadline(from: from)].compact.max,
                        claim_token: token)
    { previous: previous, token: token }
  end

  # Rolls a provisional lock back to what stood before it — but never below what a FILL has
  # confirmed, and never over somebody else's claim. The fill path runs outside the trading
  # semaphore and the semaphore itself is PER VENUE, so between this placement's claim and its
  # rollback the row can have been raised by a fill or by a second bot on another exchange. So there
  # is no read: ONE statement, floored at the row's own confirmed deadline, and guarded on the CLAIM
  # TOKEN this placement wrote. The token, not the deadline: two sales on the same day write the
  # same deadline, so matching on the value would let a failed placement roll back a second,
  # still-live one. Anyone raising the row since replaced the token, and the rollback is a no-op.
  # SQLite's scalar MAX is NULL when any argument is, hence the COALESCE; the datetimes are stored
  # as ISO text and compare as such.
  #
  # ponytail: two OVERLAPPING claims that BOTH fail can leave a deadline with no sale behind it —
  # A claims, B claims (recording A's deadline as its "previous"), A's rollback no-ops on the stale
  # token, then B restores A's deadline. The name is then locked out of buying for a window nothing
  # earned. Deliberate: the alternative error is releasing a lock that should hold, which washes a
  # real loss, and this direction costs a month of not buying one name. A claims table — one row per
  # outstanding placement, the effective deadline being the max of the live ones — removes it if it
  # ever shows up in practice.
  def restore_buy_lock!(asset_id, claim)
    return if claim.blank?

    user.wash_sale_locks
        .where(asset_id: asset_id, claim_token: claim[:token])
        .update_all(['buy_locked_until = COALESCE(MAX(confirmed_locked_until, ?), confirmed_locked_until, ?), ' \
                     'claim_token = NULL',
                     claim[:previous], claim[:previous]])
  end

  # From the fill: the sale really happened on `from`, so the window runs from there. Raises the
  # confirmed deadline and, with it, the effective one; creates a missing row; never shortens
  # either. One statement for the same reason as restore_buy_lock!. True when the effective lock
  # is now later than it was — read before the write only for the return value, which feeds a log
  # line and nothing else.
  def extend_buy_lock!(base:, from:)
    return false if wash_sale_days.zero?

    # find with a block, not find_by: the bot's tickers may be a relation or, in tests that pin them,
    # a plain Array.
    ticker = tickers.find { |t| t.base == base }
    return false if ticker.nil?

    lock = wash_sale_lock_for(ticker.base_asset_id)
    deadline = lock_deadline(from: from)
    was = lock.buy_locked_until
    # Clears the claim token too: once a fill has confirmed this deadline, no placement's rollback
    # may lower it.
    WashSaleLock.where(id: lock.id).update_all(
      ['confirmed_locked_until = COALESCE(MAX(confirmed_locked_until, ?), ?), ' \
       'buy_locked_until = COALESCE(MAX(buy_locked_until, ?), ?), claim_token = NULL',
       deadline, deadline, deadline, deadline]
    )
    was.nil? || deadline > was
  end

  # Whether the units this order submits lose money on ANY of the FIFO lots they consume — the tax
  # view, not the performance view, lot by lot rather than net, and about what is actually being
  # sold: the exchange may hold less than the position. False without a jurisdiction, so the legs
  # skip the lock bookkeeping entirely. An UNKNOWN verdict (a consumed lot whose cost we never
  # learned) is treated as a loss here as on the fill path: the provisional lock is the only
  # protection an ambiguous placement will ever get, since the resolution flow cannot reconstruct a
  # fill it never saw.
  def sell_at_loss?(order_data)
    return false if wash_sale_days.zero?

    lots = (metrics[:asset_lots] || {})[order_data[:ticker].base] || []
    Bot::TaxLots.loss_in?(lots, order_data[:amount], order_data[:quote_amount]) != false
  end

  # Says the deadline actually on the row — an earlier sale's longer window wins over this sale's.
  # Reads the TAXPAYER's lock: the composition row no longer carries one.
  def log_wash_sale_lock(base)
    ticker = tickers.find { |t| t.base == base }
    lock = ticker && user.wash_sale_locks.find_by(asset_id: ticker.base_asset_id)
    last_day = ((lock&.buy_locked_until || lock_deadline) - 1.day).to_date
    log_activity('wash_sale_locked', level: :info, details: { base: base, until: last_day.iso8601 })
  end

  # From Transaction#update_with_order_data, the ONE write path both order jobs (the single-order
  # poll and the bulk sweep) and the cancel button go through, whenever a terminal sell's status or
  # executed amounts change — closed, or cancelled after a partial fill. The metrics walk knows
  # what the fill realised against the lots. A loss on any lot creates or lengthens the lock from
  # the day the fill was SEEN — never earlier than the fill, so an observation that lags the venue
  # can only lengthen a lock; an identical re-poll saves nothing and lands nowhere near here. A sale
  # estimated as a gain that filled under water gets its lock here. Never shortens.
  #
  # NO VERDICT means the walk could not price this sale: a cancelled partial whose proceeds the
  # venue has not reported, and nothing re-polls a terminal order. Something WAS sold, so the
  # conservative reading is the lock — a window served for nothing costs a month of not buying one
  # name; a loss washed costs the loss.
  #
  # Runs inside the transaction's own save, so the lock commits with the fill or not at all.
  def reconcile_wash_sale_from_fill!(order)
    return unless order.sell? && wash_sale_days.positive?

    # The same quantity rule as the metrics walk (Transaction.confirmed_exec_amounts): a closed
    # order that reported no executed quantity executed what it asked for.
    units = order.amount_exec || (order.closed? ? order.amount : nil)
    return unless units.to_d.positive?

    verdict = (metrics(force: true)[:loss_lot_by_transaction] || {}).fetch(order.id, nil)
    return if verdict == false # nil (no proceeds known) and true both lock

    log_wash_sale_lock(order.base) if extend_buy_lock!(base: order.base, from: Time.zone.today)
  end

  # The row a lock lives on: one per taxpayer and asset, created on demand.
  def wash_sale_lock_for(asset_id)
    user.wash_sale_locks.find_or_create_by!(asset_id: asset_id)
  end

  # The locked names THIS bot has something to say about — a member or a holding it recorded. A lock
  # on an asset only another bot ever held is the account view's business, not this panel's.
  def locked_members(now: Time.current)
    return [] if wash_sale_days.zero?

    known = bot_index_assets.includes(:asset).index_by(&:asset_id)
    user.wash_sale_locks.live(now).includes(:asset).filter_map do |lock|
      row = known[lock.asset_id]
      next if row.nil?

      { symbol: lock.asset.symbol, days_left: (lock.buy_locked_until.to_date - now.to_date).to_i,
        until: lock.buy_locked_until, in_index: row.in_index }
    end
  end
end
