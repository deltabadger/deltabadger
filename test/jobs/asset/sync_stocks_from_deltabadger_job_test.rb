require 'test_helper'

class Asset::SyncStocksFromDeltabadgerJobTest < ActiveSupport::TestCase
  test 'no-op in free mode (open-source containers)' do
    MarketDataSettings.stubs(:deltabadger?).returns(false)

    MarketData.expects(:backfill_canonical_stock_external_ids!).never
    MarketData.expects(:sync_stocks_from_deltabadger!).never
    MarketData.expects(:sync_alpaca_listings_from_deltabadger!).never

    Asset::SyncStocksFromDeltabadgerJob.perform_now
  end

  test 'on hosted: runs backfill first, then stock sync, then listings sync — in that order' do
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    AppConfig.set('stock_canonical_backfill_completed_at', Time.current.iso8601)

    sequence = sequence(:hosted_sync_order)
    MarketData.expects(:backfill_canonical_stock_external_ids!).in_sequence(sequence)
    MarketData.expects(:sync_stocks_from_deltabadger!).in_sequence(sequence).returns(Result::Success.new)
    MarketData.expects(:sync_alpaca_listings_from_deltabadger!).in_sequence(sequence).returns(Result::Success.new)

    Asset::SyncStocksFromDeltabadgerJob.perform_now
  end

  test 'self-heals existing hosted containers: backfill runs and, when it sets the flag, the sync proceeds in the same invocation' do
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    begin
      AppConfig.delete('stock_canonical_backfill_completed_at')
    rescue StandardError
      nil
    end

    # Simulate a successful backfill — it sets the flag mid-invocation, so the gate in the
    # sync job opens and the stock/listings importers run on the same tick. This is the
    # whole point of in-process backfill: existing hosted containers heal without an extra
    # scheduled artifact.
    MarketData.expects(:backfill_canonical_stock_external_ids!).with do
      AppConfig.set('stock_canonical_backfill_completed_at', Time.current.iso8601)
      true
    end
    MarketData.expects(:sync_stocks_from_deltabadger!).returns(Result::Success.new)
    MarketData.expects(:sync_alpaca_listings_from_deltabadger!).returns(Result::Success.new)

    Asset::SyncStocksFromDeltabadgerJob.perform_now
  end

  test 'skips stock sync if backfill did not set the flag (avoids duplicating canonical rows)' do
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    begin
      AppConfig.delete('stock_canonical_backfill_completed_at')
    rescue StandardError
      nil
    end

    # Backfill is called but leaves the flag unset (e.g. data-api Result::Failure).
    MarketData.expects(:backfill_canonical_stock_external_ids!) # no-op stub: doesn't set the flag
    MarketData.expects(:sync_stocks_from_deltabadger!).never
    MarketData.expects(:sync_alpaca_listings_from_deltabadger!).never

    Asset::SyncStocksFromDeltabadgerJob.perform_now
  end
end
