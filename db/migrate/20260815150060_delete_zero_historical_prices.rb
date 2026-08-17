# One-off purge: a zero is never a price, it is a failed lookup wearing one. `nil.to_d` is 0, so a
# single null in an upstream prices array used to be cached and persisted as a valid price — and a
# stored zero reports the entire proceeds of a disposal as gain on a row marked complete.
#
# `Tax::PriceService` now refuses a zero at both store sites and treats one as missing when read, so
# no new zero can arrive and an existing one is no longer trusted. But it cannot leave the table on
# its own: `HistoricalPrice.store` is insert-or-ignore, and `fetch_price_range` counts a poisoned
# date as covered and returns early, so healing would trickle one row at a time through
# `fetch_single_price` forever. Deleting them lets the next report refetch the whole range at once.
#
# Raw SQL only, no models or callbacks. Idempotent.
class DeleteZeroHistoricalPrices < ActiveRecord::Migration[8.1]
  def up
    execute('DELETE FROM historical_prices WHERE price = 0')
  end

  def down
    # A deleted row is refetched on demand. Restoring the zeros would only reinstate the defect.
  end
end
