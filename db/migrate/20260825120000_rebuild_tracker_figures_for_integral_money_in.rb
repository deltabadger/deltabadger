# Money in now counts what arrived without a purchase behind it, a lost coin realises its basis,
# and a swap conserves the basis of every leg. Every stored day of history carries the old figure,
# and a stored history is only rebuilt when it has a gap — so ask for the rebuild once, per user
# with anything to rebuild. The backfill warms the ledger and re-records today when it finishes;
# the ledger job is enqueued as well for a user whose history starts today and never reaches
# that point.
class RebuildTrackerFiguresForIntegralMoneyIn < ActiveRecord::Migration[8.1]
  def up
    AccountTransaction.distinct.pluck(:user_id).each do |user_id|
      PortfolioSnapshot::BackfillJob.perform_later(user_id)
      Tracker::LedgerJob.perform_later(user_id)
    end
  end

  def down
    # Nothing to restore: the rows are derived, and the sweep writes them again.
  end
end
