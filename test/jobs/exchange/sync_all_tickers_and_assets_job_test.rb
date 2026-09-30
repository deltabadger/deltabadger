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

  test 'hosted: one random in-minute offset per tick, exchanges still a minute apart' do
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    create(:kraken_exchange)
    create(:binance_exchange)
    Exchange::SyncAllTickersAndAssetsJob.any_instance.expects(:rand).with(60).once.returns(37)

    freeze_time do
      Exchange::SyncAllTickersAndAssetsJob.perform_now

      ats = enqueued_jobs.select { |j| j[:job] == Exchange::SyncTickersAndAssetsJob }.map { |j| j[:at] }.sort
      assert_equal [(Time.current + 37.seconds).to_f, (Time.current + 97.seconds).to_f], ats
    end
  end

  test 'hosted: the offset stays inside the jitter window' do
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    create(:kraken_exchange)

    freeze_time do
      Exchange::SyncAllTickersAndAssetsJob.perform_now

      at = enqueued_jobs.sole[:at]
      assert_operator at, :>=, Time.current.to_f
      assert_operator at, :<, (Time.current + Exchange::SyncAllTickersAndAssetsJob::JITTER_WINDOW).to_f
    end
  end

  # Self-hosted, Umbrel and desktop instances are alone on CoinGecko: no herd, so no delay.
  test 'CoinGecko provider: no offset, schedule exactly as before' do
    MarketDataSettings.stubs(:deltabadger?).returns(false)
    create(:kraken_exchange)
    Exchange::SyncAllTickersAndAssetsJob.any_instance.expects(:rand).never

    freeze_time do
      Exchange::SyncAllTickersAndAssetsJob.perform_now

      assert_in_delta Time.current.to_f, enqueued_jobs.sole[:at], 0.001
    end
  end
end
