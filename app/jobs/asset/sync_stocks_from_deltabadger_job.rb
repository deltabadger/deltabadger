class Asset::SyncStocksFromDeltabadgerJob < ApplicationJob
  queue_as :default

  # The `jitter: true` dispatch run takes a SEPARATE lock from the sync it schedules. It does no
  # syncing — it only enqueues — so serialising it against the real sync is both meaningless and
  # actively harmful: it holds `sync_stocks_from_deltabadger` for the duration of its own perform,
  # and anything that made the deferred job due while that lock is held would be DESTROYED by
  # `on_conflict: :discard`, silently costing the container its whole daily sync.
  # (Proc keys are called via instance_exec(*arguments) — hence the positional-Hash form.)
  DISPATCH_LOCK = ->(*args) { args.first.is_a?(Hash) && args.first[:jitter] ? 'sync_stocks_dispatch' : 'sync_stocks_from_deltabadger' }

  limits_concurrency to: 1, key: DISPATCH_LOCK, on_conflict: :discard, duration: 1.hour

  # Fix B: a single transient data-api timeout used to drop the whole day's sync (no retry), which —
  # combined with a blanked availability table — stranded a container's stock bots at AV=0. Retry
  # transient failures with backoff so one slow/timed-out call doesn't lose the tick.
  retry_on Client::TransientNetworkError, wait: :polynomially_longer, attempts: 5
  retry_on Client::RateLimitedError, wait: :polynomially_longer, attempts: 5

  # Every hosted container fires this from the SAME cron minute, and each run pulls two bulk
  # payloads from data-api (~12 MB of stock assets, then the Alpaca listings). At ~92 containers
  # that is a thundering herd: on 2026-07-29 the 10:00 UTC burst took data-api 58s to drain through
  # its Puma threads while every container gave up at its read timeout, so 38 of 92 had to retry.
  # Spreading the actual work across a window turns the spike into a trickle.
  #
  # The window MUST close well before 10:30 UTC — Index::SyncFromCoingeckoJob runs then and depends
  # on this job's assets already being in place (see config/recurring.yml).
  JITTER_WINDOW = 15.minutes

  # Keeps the on-time path strictly in the future. Solid Queue's Job#prepare_for_execution
  # dispatches an already-due job INLINE, and Job#dispatch destroys it when the concurrency lock is
  # taken (`on_conflict: :discard`) — so before DISPATCH_LOCK split the two locks, a zero draw would
  # have enqueued the sync from inside the dispatch run and lost it with no trace, no log and no
  # retry. DISPATCH_LOCK is what makes an immediate dispatch SAFE (and #tick_time relies on that for
  # late ticks); this floor keeps the ordinary path from depending on it at all.
  MIN_JITTER = 1.second

  # `jitter: true` comes only from config/recurring.yml. The cron tick lands on every container in
  # the same second, so that path schedules the real run at a random offset rather than doing the
  # work inline. Defaults to false so perform_now and manual/console invocations run immediately,
  # and so the retry chain (which replays the original arguments) never re-defers.
  def perform(jitter: false)
    # `stock_sync_enabled` is a per-container EMERGENCY OFF SWITCH, default ON.
    # Data-api is now FIGI-canonical (no shared-ISIN collapse) and the backfill below carries
    # the ambiguity/defensive-skip guards, so stock sync runs by default. If a future data
    # issue ever recurs, an operator can freeze sync on a SINGLE container by setting
    # AppConfig.set('stock_sync_enabled', 'false'); any other value (incl. unset) means on.
    # Origin: the 2026-05-28 incident, where this began life default-off. See
    # app/models/market_data.rb for the backfill guards.
    return if AppConfig.get('stock_sync_enabled').to_s == 'false'

    return unless MarketDataSettings.deltabadger?

    # After the cheap guards, so self-hosted or frozen containers don't enqueue a job whose only
    # job is to no-op fifteen minutes later.
    return enqueue_jittered_run if jitter

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

  private

  # Random rather than derived from a container identity: the fleet self-balances without needing a
  # stable seed, and a fresh draw each day stops one unlucky container from owning the same
  # congested slot forever.
  def enqueue_jittered_run
    target = tick_time + jitter_delay
    Rails.logger.info "[SyncStocks] deferring the daily sync to #{target.utc.iso8601} to spread fleet load"
    self.class.set(wait_until: target).perform_later(jitter: false)
  end

  # Anchor the offset to the CRON TICK, not to whenever this dispatch run was picked up. The
  # default queue can be backlogged or mid-restart, starting the 10:00 dispatcher at (say) 10:20;
  # offsetting from `now` would then schedule the sync as late as 10:35, past the 10:30 index sync
  # that needs these assets — the very staleness the schedule is ordered to avoid. Anchored, a late
  # tick yields a target in the past, which enqueues the sync immediately. That is the right answer
  # (we are already late, stop adding delay) and it is safe precisely because DISPATCH_LOCK keeps
  # this run's lock separate from the sync's.
  def tick_time
    scheduled_at || enqueued_at || Time.current
  end

  def jitter_delay
    rand(MIN_JITTER.to_i..JITTER_WINDOW.to_i).seconds
  end
end
