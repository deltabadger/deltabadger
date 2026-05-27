require 'test_helper'

class Setup::SeedAndSyncJobTest < ActiveSupport::TestCase
  setup do
    Rails.application.stubs(:load_seed)
    # Avoid hitting external services from the unrelated branches of the job.
    Exchange.stubs(:available).returns([])
  end

  test 'hosted setup enqueues Asset::SyncStocksFromDeltabadgerJob after assets/indices sync' do
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    MarketData.stubs(:sync_assets!).returns(Result::Success.new)
    MarketData.stubs(:sync_indices!).returns(Result::Success.new)

    Asset::SyncStocksFromDeltabadgerJob.expects(:perform_later).once
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
