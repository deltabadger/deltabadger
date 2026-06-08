class Asset::SyncStocksFromDeltabadgerJob < ApplicationJob
  queue_as :default
  limits_concurrency to: 1, key: 'sync_stocks_from_deltabadger', on_conflict: :discard, duration: 1.hour

  # Fix B: a single transient data-api timeout used to drop the whole day's sync (no retry), which —
  # combined with a blanked availability table — stranded a container's stock bots at AV=0. Retry
  # transient failures with backoff so one slow/timed-out call doesn't lose the tick.
  retry_on Client::TransientNetworkError, wait: :polynomially_longer, attempts: 5
  retry_on Client::RateLimitedError, wait: :polynomially_longer, attempts: 5

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

    # Abort the tick if the stock-asset sync failed — running the listings sync against a
    # half-synced asset table risks importing tickers whose base assets aren't there yet. Transient
    # failures raise (caught by retry_on above); a non-transient Result::Failure stops here quietly.
    stock_result = MarketData.sync_stocks_from_deltabadger!
    unless stock_result.success?
      Rails.logger.warn "[SyncStocks] stock asset sync failed, skipping listings sync: #{stock_result.errors.to_sentence}"
      return
    end

    MarketData.sync_alpaca_listings_from_deltabadger!
  end
end
