# The invested figure on a snapshot now includes the cash a venue spent without ever reporting where
# it came from, and a row already written cannot know that. Snapshots are derived from the ledger
# and the price table, so the cheapest correct migration is to drop them: the tracker rebuilds the
# history it finds missing the next time the page is opened.
class RebuildPortfolioSnapshotsForUnfundedCash < ActiveRecord::Migration[8.1]
  def up
    execute 'DELETE FROM portfolio_snapshots'
  end

  def down
    # Nothing to restore: the rows are derived, and the sweep writes them again.
  end
end
