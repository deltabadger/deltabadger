class Asset::SyncStocksFromDeltabadgerJob < ApplicationJob
  queue_as :default
  limits_concurrency to: 1, key: 'sync_stocks_from_deltabadger', on_conflict: :discard, duration: 1.hour

  def perform
    # Post-incident 2026-05-28 kill switch — default-disabled, explicit opt-in only.
    # Even on hosted, this job is a no-op unless an operator has explicitly set
    # AppConfig.set('stock_sync_enabled', 'true') for THIS container. Guards against the
    # destructive backfill firing unsupervised after the data-api identifier-accumulation
    # bug. To re-enable, set the flag explicitly once data-api is verified clean.
    return if AppConfig.get('stock_sync_enabled').to_s != 'true'

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
