# `tickers.ath` is an incremental maximum, which only holds while a price history is immutable.
# Alpaca's stock bars were fetched unadjusted, so every corporate action left a pre-split high
# standing in units that no longer exist — and the same request was silently capped at Alpaca's
# default page, so the seeded value came from a window that ended years ago either way.
#
# Nulling the column re-seeds it from the full, split-adjusted history on the next read. Crypto
# tickers on the same venue are left alone: they have no corporate actions, and their stored
# maximum is still a maximum.
class ClearRestatedStockAths < ActiveRecord::Migration[8.1]
  def up
    Ticker.where.not(ath: nil)
          .where(exchange: Exchange.where(type: 'Exchanges::Alpaca'))
          .where.not(base_asset: Asset.where(category: 'Cryptocurrency'))
          .update_all(ath: nil, ath_updated_at: nil)
  end

  def down
    # Nothing to restore: the values were computed on a basis that no longer exists.
  end
end
