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

  included do
    store_accessor :settings, :wash_sale_enabled, :wash_sale_jurisdiction

    validates :wash_sale_jurisdiction,
              inclusion: { in: ->(_bot) { Tax::Jurisdictions.wash_sale_options.map(&:first) } },
              allow_blank: true

    # Outermost, like Bot::Rebalanceable's: the inner decorators .compact their result, which would
    # strip a deliberate "unchecked" false back to "no change".
    prepend(Module.new do
      def parse_params(params)
        parsed = super
        return parsed unless params.respond_to?(:key?)

        parsed[:wash_sale_enabled] = params[:wash_sale_enabled].presence&.in?(%w[1 true]) || false if params.key?(:wash_sale_enabled)
        parsed[:wash_sale_jurisdiction] = params[:wash_sale_jurisdiction].presence if params.key?(:wash_sale_jurisdiction)
        parsed
      end
    end)
  end

  def wash_sale_enabled?
    ActiveModel::Type::Boolean.new.cast(wash_sale_enabled).present?
  end

  # Reader fallback, never a persisted default (the Bot::Rebalanceable pattern): the select always
  # submits a value, so it needs one to render before the user has chosen, and writing one on load
  # would dirty `settings` and trip Accountable#check_missed_quote_amount_was_set on the next save.
  def wash_sale_jurisdiction
    super.presence || Tax::Jurisdictions.wash_sale_options.first.first
  end

  # Days a sold-at-a-loss constituent stays locked; 0 while the rule is switched off. Every leg
  # gates on this, so "off" and "no window" are the same state to all of them.
  def wash_sale_days
    return 0 unless wash_sale_enabled?

    Tax::Jurisdictions.for(wash_sale_jurisdiction)&.dig(:wash_sale_days).to_i
  end

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

  # Locks the constituent through the window that starts on `from`, never pulling an existing
  # deadline in, and returns the previous deadline so the caller can put it back if the sale it
  # guards provably never left (restore_buy_lock!). nil without a jurisdiction.
  def lock_buying!(asset_id, ticker:, from: Time.zone.today)
    return if wash_sale_days.zero?

    bia = lock_row_for(asset_id, ticker)
    previous = bia.buy_locked_until
    bia.update_column(:buy_locked_until, [previous, lock_deadline(from: from)].compact.max)
    previous
  end

  # Rolls a provisional lock back to what stood before it — but never below what a FILL has
  # confirmed. The fill path runs outside the trading semaphore, so a fill can land between a
  # placement's provisional extension and its rollback, and even between a rollback's read and its
  # write; so there is no read. ONE statement, and the floor is taken from the row inside it.
  # SQLite's scalar MAX is NULL when any argument is, hence the COALESCE; the datetimes are stored
  # as ISO text and compare as such.
  def restore_buy_lock!(asset_id, previous)
    bot_index_assets.where(asset_id: asset_id).update_all(
      ['buy_locked_until = COALESCE(MAX(confirmed_locked_until, ?), confirmed_locked_until, ?)', previous, previous]
    )
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

    bia = lock_row_for(ticker.base_asset_id, ticker)
    deadline = lock_deadline(from: from)
    was = bia.buy_locked_until
    bot_index_assets.where(id: bia.id).update_all(
      ['confirmed_locked_until = COALESCE(MAX(confirmed_locked_until, ?), ?), buy_locked_until = COALESCE(MAX(buy_locked_until, ?), ?)',
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
  def log_wash_sale_lock(base)
    row = bot_index_assets.includes(:asset).find { |bia| bia.asset.symbol == base }
    last_day = ((row&.buy_locked_until || lock_deadline) - 1.day).to_date
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

  # The row a lock lives on. A holding the composition never recorded — seeded by hand, or a legacy
  # row — is sellable all the same, and its lock needs a home, or the day it enters the index it
  # is bought back unprotected. Created as a quitter; the next composition refresh flips in_index
  # if it belongs (update_bot_index_assets never touches buy_locked_until).
  def lock_row_for(asset_id, ticker)
    bot_index_assets.find_or_create_by!(asset_id: asset_id) do |row|
      row.ticker = ticker
      row.in_index = false
      row.exited_at = Time.current
    end
  end

  # Every constituent under a lock, member or quitter, as the tables render it. days_left is the
  # number of days until buying resumes.
  def locked_members(now: Time.current)
    bot_index_assets.where('buy_locked_until > ?', now).includes(:asset).map do |bia|
      { symbol: bia.asset.symbol, days_left: (bia.buy_locked_until.to_date - now.to_date).to_i,
        until: bia.buy_locked_until, in_index: bia.in_index }
    end
  end
end
