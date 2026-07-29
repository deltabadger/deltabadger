require 'test_helper'

class Asset::SyncStocksFromDeltabadgerJobTest < ActiveSupport::TestCase
  setup do
    # Default state for these tests: flag unset (= default ON, post-incident re-enable),
    # deltabadger mode, backfill-completion flag cleared unless a test sets it.
    begin
      AppConfig.delete('stock_sync_enabled')
    rescue StandardError
      nil
    end
    begin
      AppConfig.delete(MarketData::STOCK_CANONICAL_BACKFILL_FLAG)
    rescue StandardError
      nil
    end
    MarketDataSettings.stubs(:deltabadger?).returns(true)
  end

  # --- Emergency off switch (post-incident 2026-05-28, now default ON) --------------------
  # `stock_sync_enabled` is a per-container emergency off switch. Default is ON now that
  # data-api is FIGI-canonical; only the exact string 'false' disables the job.

  test 'default ON: runs the backfill when the flag is unset' do
    MarketData.expects(:backfill_canonical_stock_external_ids!).once
    Asset::SyncStocksFromDeltabadgerJob.perform_now
  end

  test "emergency off: the exact string 'false' makes the job a no-op" do
    AppConfig.set('stock_sync_enabled', 'false')

    MarketData.expects(:backfill_canonical_stock_external_ids!).never
    MarketData.expects(:sync_stocks_from_deltabadger!).never
    MarketData.expects(:sync_alpaca_listings_from_deltabadger!).never

    Asset::SyncStocksFromDeltabadgerJob.perform_now
  end

  test "explicit 'true' runs the job" do
    AppConfig.set('stock_sync_enabled', 'true')
    MarketData.expects(:backfill_canonical_stock_external_ids!).once
    Asset::SyncStocksFromDeltabadgerJob.perform_now
  end

  test "a non-'false' value (e.g. capitalized 'False') still runs — only exact 'false' disables" do
    AppConfig.set('stock_sync_enabled', 'False')
    MarketData.expects(:backfill_canonical_stock_external_ids!).once
    Asset::SyncStocksFromDeltabadgerJob.perform_now
  end

  test 'no-op in free mode (open-source containers)' do
    MarketDataSettings.stubs(:deltabadger?).returns(false)

    MarketData.expects(:backfill_canonical_stock_external_ids!).never
    MarketData.expects(:sync_stocks_from_deltabadger!).never
    MarketData.expects(:sync_alpaca_listings_from_deltabadger!).never

    Asset::SyncStocksFromDeltabadgerJob.perform_now
  end

  # --- Backfill-completion gate on the stock/listings importers ---------------------------

  test 'on hosted: runs backfill first, then stock sync, then listings sync — in that order' do
    AppConfig.set(MarketData::STOCK_CANONICAL_BACKFILL_FLAG, Time.current.iso8601)

    sequence = sequence(:hosted_sync_order)
    MarketData.expects(:backfill_canonical_stock_external_ids!).in_sequence(sequence)
    MarketData.expects(:sync_stocks_from_deltabadger!).in_sequence(sequence).returns(Result::Success.new)
    MarketData.expects(:sync_alpaca_listings_from_deltabadger!).in_sequence(sequence).returns(Result::Success.new)

    Asset::SyncStocksFromDeltabadgerJob.perform_now
  end

  test 'self-heals existing containers: backfill sets the flag mid-invocation, then sync proceeds same tick' do
    MarketData.expects(:backfill_canonical_stock_external_ids!).with do
      AppConfig.set(MarketData::STOCK_CANONICAL_BACKFILL_FLAG, Time.current.iso8601)
      true
    end
    MarketData.expects(:sync_stocks_from_deltabadger!).returns(Result::Success.new)
    MarketData.expects(:sync_alpaca_listings_from_deltabadger!).returns(Result::Success.new)

    Asset::SyncStocksFromDeltabadgerJob.perform_now
  end

  test 'skips stock sync if backfill did not set the flag (avoids duplicating canonical rows)' do
    # Backfill runs but leaves the flag unset (e.g. data-api failure or unresolved legacy rows).
    MarketData.expects(:backfill_canonical_stock_external_ids!) # no-op stub: doesn't set the flag
    MarketData.expects(:sync_stocks_from_deltabadger!).never
    MarketData.expects(:sync_alpaca_listings_from_deltabadger!).never

    Asset::SyncStocksFromDeltabadgerJob.perform_now
  end

  # Fix B: a failed stock sync must abort the tick — don't run the listings sync against
  # half-synced assets.
  test 'Fix B: does not run listings sync when stock sync returns a failure' do
    AppConfig.set(MarketData::STOCK_CANONICAL_BACKFILL_FLAG, Time.current.iso8601)
    MarketData.stubs(:backfill_canonical_stock_external_ids!)
    MarketData.stubs(:sync_stocks_from_deltabadger!).returns(Result::Failure.new('boom'))
    MarketData.expects(:sync_alpaca_listings_from_deltabadger!).never

    Asset::SyncStocksFromDeltabadgerJob.perform_now
  end
end

# Retry behaviour needs the :test adapter (suite default is SolidQueue, which doesn't record
# enqueue assertions), so it lives in its own class with the adapter swapped.
class Asset::SyncStocksFromDeltabadgerJobRetryTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper

  setup do
    @old_adapter = ActiveJob::Base.queue_adapter
    ActiveJob::Base.queue_adapter = :test
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    AppConfig.set(MarketData::STOCK_CANONICAL_BACKFILL_FLAG, Time.current.iso8601)
    MarketData.stubs(:backfill_canonical_stock_external_ids!)
  end

  teardown { ActiveJob::Base.queue_adapter = @old_adapter }

  # Fix B: the prod outage — a single Net::ReadTimeout dropped the whole day's sync because the job
  # had no retry_on. It must now retry instead of dying.
  test 'Fix B: retries on a transient network error instead of dropping the job' do
    MarketData.stubs(:sync_stocks_from_deltabadger!).raises(Client::TransientNetworkError, 'Net::ReadTimeout')

    assert_enqueued_jobs 1, only: Asset::SyncStocksFromDeltabadgerJob do
      Asset::SyncStocksFromDeltabadgerJob.perform_now
    end
  end

  test 'retries on a rate-limited error instead of dropping the job' do
    MarketData.stubs(:sync_stocks_from_deltabadger!).raises(Client::RateLimitedError, 'rate limited')

    assert_enqueued_jobs 1, only: Asset::SyncStocksFromDeltabadgerJob do
      Asset::SyncStocksFromDeltabadgerJob.perform_now
    end
  end
end

# Fleet-wide load spreading. Every hosted container shares the same cron minute, so the recurring
# tick must NOT do the work inline — it schedules it at a random offset instead.
class Asset::SyncStocksFromDeltabadgerJobJitterTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper

  setup do
    @old_adapter = ActiveJob::Base.queue_adapter
    ActiveJob::Base.queue_adapter = :test
    begin
      AppConfig.delete('stock_sync_enabled')
    rescue StandardError
      nil
    end
    MarketDataSettings.stubs(:deltabadger?).returns(true)
  end

  teardown { ActiveJob::Base.queue_adapter = @old_adapter }

  test 'jitter: true defers the work instead of doing it inline' do
    MarketData.expects(:backfill_canonical_stock_external_ids!).never
    MarketData.expects(:sync_stocks_from_deltabadger!).never
    MarketData.expects(:sync_alpaca_listings_from_deltabadger!).never

    assert_enqueued_with(job: Asset::SyncStocksFromDeltabadgerJob, args: [{ jitter: false }]) do
      Asset::SyncStocksFromDeltabadgerJob.perform_now(jitter: true)
    end
  end

  test 'the deferred run is scheduled inside the jitter window' do
    freeze_time do
      Asset::SyncStocksFromDeltabadgerJob.perform_now(jitter: true)

      scheduled_at = enqueued_jobs.sole[:at]
      assert_operator scheduled_at, :>=, Time.current.to_f
      # Inclusive: the draw's upper bound IS the window, so equality is a legal 1-in-900 outcome.
      assert_operator scheduled_at, :<=, (Time.current + Asset::SyncStocksFromDeltabadgerJob::JITTER_WINDOW).to_f
    end
  end

  # --- The window is measured from the tick, not from pickup ---------------------------------
  #
  # config/recurring.yml orders the 10:00 stock sync before the 10:30 index sync and documents that
  # inverting them serves a stale index for a full day. A backlogged default queue can start this
  # dispatch run well after 10:00; offsetting from `now` would silently spend the ordering margin.

  test 'the jitter is anchored to the cron tick, not to when the dispatch run was picked up' do
    freeze_time do
      tick = Time.current - 10.minutes
      Asset::SyncStocksFromDeltabadgerJob.any_instance.stubs(:jitter_delay).returns(60.seconds)

      job = Asset::SyncStocksFromDeltabadgerJob.new(jitter: true)
      job.enqueued_at = tick
      job.perform_now

      assert_in_delta (tick + 60.seconds).to_f, enqueued_jobs.sole[:at], 1,
                      'the offset must run from the 10:00 tick, not from a late pickup'
    end
  end

  test 'a dispatch run picked up after the whole window has passed stops deferring' do
    freeze_time do
      job = Asset::SyncStocksFromDeltabadgerJob.new(jitter: true)
      job.enqueued_at = Time.current - (Asset::SyncStocksFromDeltabadgerJob::JITTER_WINDOW + 5.minutes)
      job.perform_now

      assert_operator enqueued_jobs.sole[:at], :<=, Time.current.to_f,
                      'already late — further delay would push the sync past the 10:30 index sync'
    end
  end

  # --- Never let the deferred sync be discarded ---------------------------------------------
  #
  # Solid Queue's Job#prepare_for_execution dispatches an already-due job INLINE, and Job#dispatch
  # destroys it when the concurrency lock is taken (`on_conflict: :discard`). A zero-second jitter
  # draw therefore enqueues the sync from inside the dispatch run and loses it with no trace, no
  # log and no retry — a container silently skipping its whole day. Two independent guarantees
  # below: separate locks, and a non-zero floor.

  test 'the dispatch run and the sync it schedules take different concurrency locks' do
    dispatch = Asset::SyncStocksFromDeltabadgerJob.new(jitter: true)
    sync = Asset::SyncStocksFromDeltabadgerJob.new

    refute_equal dispatch.concurrency_key, sync.concurrency_key
    # The real sync's lock is unchanged from before jitter existed.
    assert_equal 'Asset::SyncStocksFromDeltabadgerJob/sync_stocks_from_deltabadger', sync.concurrency_key
  end

  test 'the jitter delay is drawn from a range that excludes zero' do
    job = Asset::SyncStocksFromDeltabadgerJob.new
    job.expects(:rand).with(1..Asset::SyncStocksFromDeltabadgerJob::JITTER_WINDOW.to_i).returns(1)

    assert_equal 1.second, job.send(:jitter_delay)
  end

  test 'the deferred run is always scheduled strictly in the future, even on the smallest draw' do
    Asset::SyncStocksFromDeltabadgerJob.any_instance.stubs(:jitter_delay)
                                       .returns(Asset::SyncStocksFromDeltabadgerJob::MIN_JITTER)

    freeze_time do
      Asset::SyncStocksFromDeltabadgerJob.perform_now(jitter: true)

      assert_operator enqueued_jobs.sole[:at], :>, Time.current.to_f,
                      'a due-on-arrival job is dispatched inline and destroyed by on_conflict: :discard'
    end
  end

  # config/recurring.yml runs Index::SyncFromCoingeckoJob at 10:30 UTC and documents that it MUST
  # land after this job's assets. The jitter window has to close with room to spare, retries
  # included — widening it past that would serve a stale index for a full day.
  test 'the jitter window closes well before the 10:30 UTC index sync that depends on it' do
    assert_operator Asset::SyncStocksFromDeltabadgerJob::JITTER_WINDOW, :<=, 20.minutes
  end

  test 'the default (no args) does the work immediately — jitter is opt-in from recurring.yml' do
    AppConfig.set(MarketData::STOCK_CANONICAL_BACKFILL_FLAG, Time.current.iso8601)
    MarketData.expects(:backfill_canonical_stock_external_ids!).once
    MarketData.expects(:sync_stocks_from_deltabadger!).returns(Result::Success.new)
    MarketData.expects(:sync_alpaca_listings_from_deltabadger!).returns(Result::Success.new)

    assert_no_enqueued_jobs do
      Asset::SyncStocksFromDeltabadgerJob.perform_now
    end
  end

  # Deferring first would have every self-hosted container enqueue a job that only no-ops 15
  # minutes later.
  test 'free-mode containers enqueue nothing at all' do
    MarketDataSettings.stubs(:deltabadger?).returns(false)

    assert_no_enqueued_jobs do
      Asset::SyncStocksFromDeltabadgerJob.perform_now(jitter: true)
    end
  end

  test 'the emergency off switch short-circuits before anything is enqueued' do
    AppConfig.set('stock_sync_enabled', 'false')

    assert_no_enqueued_jobs do
      Asset::SyncStocksFromDeltabadgerJob.perform_now(jitter: true)
    end
  end

  # The wiring is the whole point: without `args` the cron tick would still sync inline on every
  # container at the same second.
  test 'recurring.yml drives the daily run through the jitter path in every environment' do
    config = ActiveSupport::ConfigurationFile.parse(Rails.root.join('config/recurring.yml')).deep_symbolize_keys

    %i[production development].each do |env|
      task = config.dig(env, :sync_stocks_from_deltabadger_job)
      assert_equal [{ jitter: true }], task[:args], "#{env} must defer the fleet-wide burst"
      assert_equal '0 10 * * *', task[:schedule], "#{env} schedule changed — recheck the 10:30 index dependency"
    end
  end

  # The load bearing detail nothing else covers: a YAML `args` hash only becomes Ruby KEYWORDS via
  # SolidQueue::RecurringTask#arguments_with_kwargs (Hash.ruby2_keywords_hash) and survives the
  # ActiveJob serialize/deserialize round-trip only if that flag is preserved. If either end of
  # that bridge breaks, `perform` gets a positional Hash — an ArgumentError at best, and at worst a
  # silent fall-through to jitter: false that puts the whole fleet back on the same second.
  test 'the recurring args reach perform as keywords, through the full Solid Queue round-trip' do
    config = ActiveSupport::ConfigurationFile.parse(Rails.root.join('config/recurring.yml')).deep_symbolize_keys
    entry = config.dig(:production, :sync_stocks_from_deltabadger_job)
    task = SolidQueue::RecurringTask.from_configuration(:sync_stocks_from_deltabadger_job, **entry)

    job = Asset::SyncStocksFromDeltabadgerJob.new(*task.send(:arguments_with_kwargs))
    round_tripped = ActiveJob::Base.deserialize(job.serialize)
    round_tripped.send(:deserialize_arguments_if_needed)

    assert Hash.ruby2_keywords_hash?(round_tripped.arguments.last),
           'the kwargs flag was lost — perform would receive a positional Hash'

    MarketData.expects(:sync_stocks_from_deltabadger!).never
    assert_enqueued_with(job: Asset::SyncStocksFromDeltabadgerJob, args: [{ jitter: false }]) do
      round_tripped.perform_now
    end
  end
end
