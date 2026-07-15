require 'test_helper'

class Asset::SyncAlpacaCryptoFromDeltabadgerJobTest < ActiveSupport::TestCase
  test 'does nothing in free mode' do
    MarketDataSettings.stubs(:deltabadger?).returns(false)
    MarketData.expects(:sync_alpaca_crypto_listings_from_deltabadger!).never

    Asset::SyncAlpacaCryptoFromDeltabadgerJob.perform_now
  end

  test 'syncs alpaca crypto listings in hosted mode' do
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    MarketData.expects(:sync_alpaca_crypto_listings_from_deltabadger!).returns(Result::Success.new)

    Asset::SyncAlpacaCryptoFromDeltabadgerJob.perform_now
  end

  test 'logs a warning without raising when the sync fails' do
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    MarketData.stubs(:sync_alpaca_crypto_listings_from_deltabadger!).returns(Result::Failure.new('boom'))

    assert_nothing_raised { Asset::SyncAlpacaCryptoFromDeltabadgerJob.perform_now }
  end
end

# Retry behaviour needs the :test adapter (suite default is SolidQueue, which doesn't record
# enqueue assertions) — mirrors Asset::SyncStocksFromDeltabadgerJobRetryTest exactly.
class Asset::SyncAlpacaCryptoFromDeltabadgerJobRetryTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper

  setup do
    @old_adapter = ActiveJob::Base.queue_adapter
    ActiveJob::Base.queue_adapter = :test
    MarketDataSettings.stubs(:deltabadger?).returns(true)
  end

  teardown { ActiveJob::Base.queue_adapter = @old_adapter }

  test 'retries on a transient network error instead of dropping the job' do
    MarketData.stubs(:sync_alpaca_crypto_listings_from_deltabadger!).raises(Client::TransientNetworkError, 'Net::ReadTimeout')

    assert_enqueued_jobs 1, only: Asset::SyncAlpacaCryptoFromDeltabadgerJob do
      Asset::SyncAlpacaCryptoFromDeltabadgerJob.perform_now
    end
  end

  test 'retries on a rate-limited error instead of dropping the job' do
    MarketData.stubs(:sync_alpaca_crypto_listings_from_deltabadger!).raises(Client::RateLimitedError, 'rate limited')

    assert_enqueued_jobs 1, only: Asset::SyncAlpacaCryptoFromDeltabadgerJob do
      Asset::SyncAlpacaCryptoFromDeltabadgerJob.perform_now
    end
  end
end
