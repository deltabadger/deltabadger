# QUICK now means the pre-redenomination token (`quick`) through 2023-07-20 (`Tax::AssetIdentity`).
# Prices stored for those days were fetched as the new token, 1000x too low, and storage is
# insert-only, so they are cleared to be fetched under the coin each day meant. A complete portfolio
# history is never swept again on its own, so every history holding QUICK is rebuilt — the backfill
# recomputes the ledgers when it is done.
class RefetchQuickUnderTheOldToken < ActiveRecord::Migration[8.1]
  def up
    HistoricalPrice.where(asset: 'QUICK', date: ..Date.new(2023, 7, 20)).delete_all
    AccountTransaction.where('? IN (base_currency, quote_currency, fee_currency)', 'QUICK')
                      .distinct.pluck(:user_id).each { |user_id| PortfolioSnapshot::BackfillJob.perform_later(user_id) }
  end

  def down
    # Nothing to restore: the rows are reference data, and the next report fetches them again.
  end
end
