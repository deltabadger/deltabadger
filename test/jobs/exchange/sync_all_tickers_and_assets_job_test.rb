require 'test_helper'

class Exchange::SyncAllTickersAndAssetsJobTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper

  setup do
    @original_adapter = ActiveJob::Base.queue_adapter
    ActiveJob::Base.queue_adapter = :test
    MarketData.stubs(:configured?).returns(true)
  end

  teardown do
    ActiveJob::Base.queue_adapter = @original_adapter
  end

  test 'enqueues a sync job for crypto exchanges' do
    kraken = create(:kraken_exchange)

    assert_enqueued_with(job: Exchange::SyncTickersAndAssetsJob, args: [kraken]) do
      Exchange::SyncAllTickersAndAssetsJob.perform_now
    end
  end

  test 'does not enqueue sync jobs for stock venues' do
    create(:alpaca_exchange)
    create(:ibkr_exchange)

    assert_no_enqueued_jobs(only: Exchange::SyncTickersAndAssetsJob) do
      Exchange::SyncAllTickersAndAssetsJob.perform_now
    end
  end
end
