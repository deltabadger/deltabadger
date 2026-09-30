class Asset::FetchAllAssetsDataFromCoingeckoJob < ApplicationJob
  queue_as :low_priority

  # The `jitter: true` run never fetches — it only schedules the fetch — so it takes its own lock:
  # sharing the fetch's would let `on_conflict: :discard` destroy a fetch dispatched while this run
  # still held it (see Asset::SyncStocksFromDeltabadgerJob::DISPATCH_LOCK).
  # (Proc keys are called via instance_exec(*arguments) — hence the positional-Hash form.)
  DISPATCH_LOCK = lambda { |*args|
    args.first.is_a?(Hash) && args.first[:jitter] ? 'fetch_all_assets_data_dispatch' : 'fetch_all_assets_data_from_coingecko'
  }

  limits_concurrency to: 1, key: DISPATCH_LOCK, on_conflict: :discard, duration: 1.hour

  # Instances sharing one market-data server all tick at 00:20:00, so the fetch is spread across a
  # window. It must still land before the 00:30 rebalancer, which reads market caps for
  # market-cap-weighted baskets — hence a short window anchored to the tick.
  JITTER_WINDOW = 5.minutes

  # `jitter: true` comes only from config/recurring.yml. Exchange::Synchronizer's no-arg
  # perform_later, perform_now and the console all fetch immediately.
  def perform(jitter: false)
    return unless MarketData.configured?
    return enqueue_fetch if jitter

    result = MarketData.sync_assets!
    Rails.logger.warn "[MarketData] Failed to sync assets: #{result.errors.to_sentence}" if result.failure?
  rescue StandardError => e
    Rails.logger.warn "[MarketData] Error syncing assets: #{e.message}"
  end

  private

  # On the deltabadger provider the delay is random, not derived from the instance — see
  # Asset::SyncStocksFromDeltabadgerJob#enqueue_jittered_run — and starts at 1 s so an on-time run
  # is never due on arrival. On CoinGecko there is no shared server to spread load over: no delay.
  # Anchored to when this run was enqueued, so a late pickup spends the window instead of extending
  # it; a target already in the past enqueues the fetch at once, which the split lock makes safe.
  def enqueue_fetch
    delay = MarketDataSettings.deltabadger? ? rand(1..JITTER_WINDOW.to_i).seconds : 0.seconds
    self.class.set(wait_until: (scheduled_at || enqueued_at || Time.current) + delay).perform_later(jitter: false)
  end
end
