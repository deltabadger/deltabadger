class Asset::SyncStocksFromDeltabadgerJob < ApplicationJob
  queue_as :default
  limits_concurrency to: 1, key: 'sync_stocks_from_deltabadger', on_conflict: :discard, duration: 1.hour

  def perform
    return unless MarketDataSettings.deltabadger?

    # In-process backfill on every invocation: existing hosted containers heal on the
    # next recurring tick without orchestration. Idempotent (flag-checked internally).
    MarketData.backfill_canonical_stock_external_ids!

    # Gate the stock+listings sync on the backfill having succeeded; otherwise we'd risk
    # creating canonical rows alongside untouched legacy alpaca_<uuid> ones.
    return unless AppConfig.get(MarketData::STOCK_CANONICAL_BACKFILL_FLAG).present?

    MarketData.sync_stocks_from_deltabadger!
    MarketData.sync_alpaca_listings_from_deltabadger!
  end
end
