# A price stored under a symbol stays: storage is insert-only and a covered date is never fetched
# again. Prices fetched before `Tax::AssetIdentity` were fetched under whichever asset row came
# first — the relaunched Terra for 2021 LUNA, the wrong LIT — so every date an alias now speaks for
# is cleared, to be fetched under the coin the symbol meant. The ledgers are asked to recompute
# once the gaps are filled again.
class RefetchPricesUnderTheirCoin < ActiveRecord::Migration[8.1]
  def up
    Tax::AssetIdentity::ALIASES.each do |entry|
      scope = HistoricalPrice.where(asset: entry.symbol)
      scope = scope.where(date: ..entry.until) if entry.until
      scope.delete_all
    end
    AccountTransaction.distinct.pluck(:user_id).each { |user_id| Tracker::LedgerJob.perform_later(user_id) }
  end

  def down
    # Nothing to restore: the rows are reference data, and the next report fetches them again.
  end
end
