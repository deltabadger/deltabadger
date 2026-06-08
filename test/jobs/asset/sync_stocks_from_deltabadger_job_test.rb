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
end
