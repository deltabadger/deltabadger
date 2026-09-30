require 'test_helper'

class Asset::FetchAllAssetsDataFromCoingeckoJobTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper

  JOB = Asset::FetchAllAssetsDataFromCoingeckoJob

  setup do
    @old_adapter = ActiveJob::Base.queue_adapter
    ActiveJob::Base.queue_adapter = :test
    MarketData.stubs(:configured?).returns(true)
    MarketDataSettings.stubs(:deltabadger?).returns(true)
  end

  teardown { ActiveJob::Base.queue_adapter = @old_adapter }

  def dispatch_job = JOB.new(Hash.ruby2_keywords_hash({ jitter: true }))

  test 'hosted, jitter: true defers the fetch instead of doing it inline' do
    MarketData.expects(:sync_assets!).never

    assert_enqueued_with(job: JOB, args: [{ jitter: false }]) do
      JOB.perform_now(jitter: true)
    end
  end

  test 'the deferred fetch lands strictly in the future and inside the window' do
    freeze_time do
      JOB.perform_now(jitter: true)

      at = enqueued_jobs.sole[:at]
      assert_operator at, :>, Time.current.to_f
      assert_operator at, :<=, (Time.current + JOB::JITTER_WINDOW).to_f
    end
  end

  test 'the jitter delay is drawn from a range that excludes zero' do
    job = dispatch_job
    job.expects(:rand).with(1..JOB::JITTER_WINDOW.to_i).returns(1)

    freeze_time do
      job.perform_now
      assert_in_delta (Time.current + 1.second).to_f, enqueued_jobs.sole[:at], 0.001
    end
  end

  test 'the offset runs from when the tick enqueued this run, not from a late pickup' do
    freeze_time do
      job = dispatch_job
      job.enqueued_at = Time.current - 2.minutes
      job.stubs(:rand).returns(60)
      job.perform_now

      assert_in_delta (Time.current - 1.minute).to_f, enqueued_jobs.sole[:at], 1
    end
  end

  test 'a pickup after the whole window has passed fetches as soon as possible' do
    freeze_time do
      job = dispatch_job
      job.enqueued_at = Time.current - (JOB::JITTER_WINDOW + 5.minutes)
      job.perform_now

      assert_operator enqueued_jobs.sole[:at], :<=, Time.current.to_f
    end
  end

  # The 00:30 rebalancer reads Asset.market_cap for market-cap-weighted baskets. The fetch must
  # land before it with time to run, as it did when it ran inline at 00:20.
  test 'the window closes with room to spare before the 00:30 rebalancer' do
    config = ActiveSupport::ConfigurationFile.parse(Rails.root.join('config/recurring.yml')).deep_symbolize_keys

    assert_operator JOB::JITTER_WINDOW, :<=, 5.minutes
    %i[production development].each do |env|
      assert_equal '20 0 * * *', config.dig(env, :fetch_all_assets_data_from_coingecko_job, :schedule),
                   "#{env}: fetch schedule moved — recheck against the rebalancer"
      assert_equal '30 */4 * * *', config.dig(env, :evaluate_rebalancers_job, :schedule),
                   "#{env}: rebalancer schedule moved — recheck against the asset fetch"
    end
  end

  # Self-hosted, Umbrel and desktop instances are alone on CoinGecko: no delay. The fetch still runs
  # as its own job so that every fetch holds the fetch lock, whatever the provider.
  test 'CoinGecko provider: jitter: true enqueues the fetch due immediately' do
    MarketDataSettings.stubs(:deltabadger?).returns(false)
    MarketData.expects(:sync_assets!).never

    freeze_time do
      assert_enqueued_with(job: JOB, args: [{ jitter: false }]) { JOB.perform_now(jitter: true) }
      assert_operator enqueued_jobs.sole[:at], :<=, Time.current.to_f
    end
  end

  # Exchange::Synchronizer enqueues this with no args when an exchange's catalogue changes; that
  # path must stay immediate.
  test 'the default (no args) fetches immediately' do
    MarketData.expects(:sync_assets!).once.returns(Result::Success.new)

    assert_no_enqueued_jobs { JOB.perform_now }
  end

  test 'unconfigured market data: jitter: true does nothing at all' do
    MarketData.stubs(:configured?).returns(false)
    MarketData.expects(:sync_assets!).never

    assert_no_enqueued_jobs { JOB.perform_now(jitter: true) }
  end

  # on_conflict: :discard destroys a job dispatched while its lock is held. If the dispatch run
  # held the fetch's lock, a deferred fetch made due in that moment would vanish without a log.
  test 'the dispatch run and the fetch it schedules take different concurrency locks' do
    refute_equal dispatch_job.concurrency_key, JOB.new.concurrency_key
    assert_equal 'Asset::FetchAllAssetsDataFromCoingeckoJob/fetch_all_assets_data_from_coingecko', JOB.new.concurrency_key
  end

  test 'recurring.yml drives the daily fetch through the jitter path in every environment' do
    config = ActiveSupport::ConfigurationFile.parse(Rails.root.join('config/recurring.yml')).deep_symbolize_keys

    %i[production development].each do |env|
      assert_equal [{ jitter: true }], config.dig(env, :fetch_all_assets_data_from_coingecko_job, :args),
                   "#{env} must take the jitter path"
    end
  end

  # Covers argument SERIALIZATION only (Solid Queue's semaphore itself is library behaviour; the
  # key tests above pin what this app controls). A YAML `args` hash becomes Ruby keywords only
  # through SolidQueue::RecurringTask's ruby2_keywords flag, and only if it survives
  # serialize/deserialize — lose it and every instance silently falls back to jitter: false.
  test 'the recurring args reach perform as keywords after serialization' do
    config = ActiveSupport::ConfigurationFile.parse(Rails.root.join('config/recurring.yml')).deep_symbolize_keys
    entry = config.dig(:production, :fetch_all_assets_data_from_coingecko_job)
    task = SolidQueue::RecurringTask.from_configuration(:fetch_all_assets_data_from_coingecko_job, **entry)

    job = JOB.new(*task.send(:arguments_with_kwargs))
    round_tripped = ActiveJob::Base.deserialize(job.serialize)
    round_tripped.send(:deserialize_arguments_if_needed)

    assert Hash.ruby2_keywords_hash?(round_tripped.arguments.last),
           'the kwargs flag was lost — perform would receive a positional Hash'

    MarketData.expects(:sync_assets!).never
    assert_enqueued_with(job: JOB, args: [{ jitter: false }]) { round_tripped.perform_now }
  end
end
