class Asset::SyncStocksFromDeltabadgerJob < ApplicationJob
  queue_as :default
  limits_concurrency to: 1, key: 'sync_stocks_from_deltabadger', on_conflict: :discard, duration: 1.hour

  def perform
    # `stock_sync_enabled` is a per-container EMERGENCY OFF SWITCH, default ON.
    # Data-api is now FIGI-canonical (no shared-ISIN collapse) and the backfill below carries
    # the ambiguity/defensive-skip guards, so stock sync runs by default. If a future data
    # issue ever recurs, an operator can freeze sync on a SINGLE container by setting
    # AppConfig.set('stock_sync_enabled', 'false'); any other value (incl. unset) means on.
    # Origin: the 2026-05-28 incident, where this began life default-off. See
    # app/models/market_data.rb for the backfill guards.
    return if AppConfig.get('stock_sync_enabled').to_s == 'false'

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
