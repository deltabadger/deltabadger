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
    store_accessor :settings, :wash_sale_jurisdiction

    validates :wash_sale_jurisdiction,
              inclusion: { in: ->(_bot) { Tax::Jurisdictions.wash_sale_options.map(&:first) } },
              allow_blank: true

    # Outermost, like Bot::Rebalanceable's: the inner decorators .compact their result, which would
    # strip a deliberate "None" (nil) back to "no change".
    prepend(Module.new do
      def parse_params(params)
        parsed = super
        return parsed unless params.respond_to?(:key?) && params.key?(:wash_sale_jurisdiction)

        parsed.merge(wash_sale_jurisdiction: params[:wash_sale_jurisdiction].presence)
      end
    end)
  end

  # Days a sold-at-a-loss constituent stays locked; 0 when no jurisdiction is chosen.
  def wash_sale_days
    Tax::Jurisdictions.for(wash_sale_jurisdiction)&.dig(:wash_sale_days).to_i
  end

  # Buying resumes at the start of the day after the window.
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
