require 'test_helper'

class Setup::SeedAndSyncJobTest < ActiveSupport::TestCase
  setup do
    Rails.application.stubs(:load_seed)
    # Avoid hitting external services from the unrelated branches of the job.
    Exchange.stubs(:available).returns([])
  end

  # Re-enabled after the 2026-05-28 incident: hosted setup enqueues the stocks sync job so a
  # freshly provisioned container gets stocks out-of-box (data-api is now FIGI-canonical).
  # Free mode still never touches stocks.

  test 'hosted setup enqueues the stocks sync job (out-of-box stocks)' do
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    MarketData.stubs(:sync_assets!).returns(Result::Success.new)
    MarketData.stubs(:sync_indices!).returns(Result::Success.new)

    Asset::SyncStocksFromDeltabadgerJob.expects(:perform_later).once
    Setup::SeedAndSyncJob.perform_now
  end

  test 'hosted setup does not sync tickers for stock venues' do
    kraken = create(:kraken_exchange)
    ibkr = create(:ibkr_exchange)
    Exchange.stubs(:available).returns([kraken, ibkr])

    MarketDataSettings.stubs(:deltabadger?).returns(true)
    MarketData.stubs(:sync_assets!).returns(Result::Success.new)
    MarketData.stubs(:sync_indices!).returns(Result::Success.new)
    Asset::SyncStocksFromDeltabadgerJob.stubs(:perform_later)

    MarketData.expects(:sync_tickers!).with(kraken).once
    MarketData.expects(:sync_tickers!).with(ibkr).never

    Setup::SeedAndSyncJob.perform_now
  end

  test 'free-mode setup does NOT enqueue the stocks sync job' do
    MarketDataSettings.stubs(:deltabadger?).returns(false)
    # Disable the rate-limit sleep in the free-mode CoinGecko branch.
    Setup::SeedAndSyncJob.any_instance.stubs(:sleep)
    AppConfig.stubs(:coingecko_configured?).returns(false)

    Asset::SyncStocksFromDeltabadgerJob.expects(:perform_later).never
    Setup::SeedAndSyncJob.perform_now
  end
end
