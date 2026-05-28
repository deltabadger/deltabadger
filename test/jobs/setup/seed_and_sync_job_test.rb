require 'test_helper'

class Setup::SeedAndSyncJobTest < ActiveSupport::TestCase
  setup do
    Rails.application.stubs(:load_seed)
    # Avoid hitting external services from the unrelated branches of the job.
    Exchange.stubs(:available).returns([])
  end

  # Post-incident 2026-05-28: setup never auto-enqueues the stocks sync job.
  # Stock sync is opt-in via AppConfig.set('stock_sync_enabled', 'true') per container.
  # Setup just seeds the crypto/fiat universe; nothing stock-related fires automatically.

  test 'hosted setup does NOT auto-enqueue the stocks sync job (opt-in only)' do
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    MarketData.stubs(:sync_assets!).returns(Result::Success.new)
    MarketData.stubs(:sync_indices!).returns(Result::Success.new)

    Asset::SyncStocksFromDeltabadgerJob.expects(:perform_later).never
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
