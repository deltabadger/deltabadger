require 'test_helper'

class Bot::LimitCheckableTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

  setup do
    @bot = create(:dca_single_asset, :started)
    @bot.update!(status: :waiting)
  end

  test 'live_limit_check_type is nil with no limit_paused log' do
    assert_nil @bot.live_limit_check_type
  end

  test 'live_limit_check_type reads the most recent limit_paused log' do
    @bot.log_activity('limit_paused', details: { limit_type: :price })
    assert_equal 'price', @bot.live_limit_check_type
  end

  # Codex R3: "currently paused", not "ever paused". A pause OLDER than the last ActionJob run
  # means the bot has since run a normal cycle → it is NOT currently limit-paused.
  test 'live_limit_check_type is nil when the pause predates the last ActionJob run' do
    @bot.log_activity('limit_paused', details: { limit_type: :price })
    @bot.update!(last_action_job_at: Time.current + 1.second) # a normal cycle ran after the pause
    assert_nil @bot.live_limit_check_type
  end

  test 'live_limit_check_type returns the type when the pause is from the latest run' do
    @bot.update!(last_action_job_at: Time.current)
    @bot.log_activity('limit_paused', details: { limit_type: :price }) # pause is at/after last run
    assert_equal 'price', @bot.live_limit_check_type
  end

  # The KEY case (Codex High #1): indicator enabled AND satisfied, price enabled AND unmet →
  # the bot was paused by PRICE, so the live chain is PriceLimitCheckJob, NOT IndicatorLimitCheckJob.
  # We model that by the limit_paused log the decorator actually wrote (limit_type: :price).
  test 'live_limit_check_job_class follows the limit_paused log, not enabled-predicate precedence' do
    @bot.price_limited = true
    @bot.indicator_limited = true
    @bot.set_missed_quote_amount
    @bot.save!
    @bot.log_activity('limit_paused', details: { limit_type: :price })
    assert_equal Bot::PriceLimitCheckJob, @bot.live_limit_check_job_class
  end

  test 'live_limit_check_job_class maps each type to its job class' do
    # Reuse @bot's exchange/assets across iterations: Exchanges::Binance type is globally unique
    # and bitcoin/usd carry unique external_ids, so re-creating them per loop would collide.
    exchange = @bot.exchange
    bitcoin = @bot.base_asset
    usd = @bot.quote_asset
    {
      'price' => Bot::PriceLimitCheckJob,
      'price_drop' => Bot::PriceDropLimitCheckJob,
      'moving_average' => Bot::MovingAverageLimitCheckJob,
      'indicator' => Bot::IndicatorLimitCheckJob
    }.each do |type, klass|
      bot = create(:dca_single_asset, :started, exchange:, base_asset: bitcoin, quote_asset: usd)
      bot.update!(status: :waiting)
      bot.log_activity('limit_paused', details: { limit_type: type })
      assert_equal klass, bot.live_limit_check_job_class, "#{type} should map to #{klass}"
    end
  end

  test 'live_limit_check_job_class is nil when there is no limit_paused log' do
    assert_nil @bot.live_limit_check_job_class
  end

  test 'pending_limit_check_job? is false when no check job exists' do
    @bot.log_activity('limit_paused', details: { limit_type: :price })
    refute @bot.pending_limit_check_job?
  end

  test 'pending_limit_check_job? is true for a SCHEDULED check job' do
    @bot.log_activity('limit_paused', details: { limit_type: :price })
    Bot::PriceLimitCheckJob.set(wait_until: 1.minute.from_now).perform_later(@bot)
    assert @bot.pending_limit_check_job?
  end

  # Codex R2 #3: assert each ACTIVE execution state counts as pending (Ready / Claimed / Blocked).
  # We move the just-enqueued job's execution row between tables to model each state.
  test 'pending_limit_check_job? is true for a READY check job' do
    @bot.log_activity('limit_paused', details: { limit_type: :price })
    Bot::PriceLimitCheckJob.perform_later(@bot) # enqueues directly into ReadyExecution
    assert SolidQueue::ReadyExecution.joins(:job)
                                     .where(solid_queue_jobs: { class_name: 'Bot::PriceLimitCheckJob' }).exists?,
           'precondition: job is in ReadyExecution'
    assert @bot.pending_limit_check_job?
  end

  test 'pending_limit_check_job? is true for a CLAIMED (mid-run) check job' do
    @bot.log_activity('limit_paused', details: { limit_type: :price })
    Bot::PriceLimitCheckJob.perform_later(@bot)
    job_id = SolidQueue::Job.find_by(class_name: 'Bot::PriceLimitCheckJob').id
    process = SolidQueue::Process.create!(kind: 'Worker', pid: 1, name: 'test-worker', last_heartbeat_at: Time.current)
    SolidQueue::ReadyExecution.where(job_id:).delete_all
    SolidQueue::ClaimedExecution.create!(job_id:, process_id: process.id)
    assert @bot.pending_limit_check_job?
  end

  test 'pending_limit_check_job? is true for a BLOCKED check job' do
    @bot.log_activity('limit_paused', details: { limit_type: :price })
    Bot::PriceLimitCheckJob.perform_later(@bot)
    job = SolidQueue::Job.find_by(class_name: 'Bot::PriceLimitCheckJob')
    # BlockedExecution reads concurrency_key from the associated job (assumes_attributes_from_job),
    # so set it on the job rather than passing the (ignored) attribute to create!.
    job.update!(concurrency_key: 'k')
    SolidQueue::ReadyExecution.where(job_id: job.id).delete_all
    SolidQueue::BlockedExecution.create!(job_id: job.id, queue_name: 'default', expires_at: 5.minutes.from_now)
    assert @bot.pending_limit_check_job?
  end

  # A job present ONLY in FailedExecution is the dead chain we recover — must read as not-pending.
  test 'pending_limit_check_job? ignores a dead-lettered (failed-only) check job' do
    @bot.log_activity('limit_paused', details: { limit_type: :price })
    Bot::PriceLimitCheckJob.perform_later(@bot)
    job_id = SolidQueue::Job.find_by(class_name: 'Bot::PriceLimitCheckJob').id
    SolidQueue::ReadyExecution.where(job_id:).delete_all
    SolidQueue::FailedExecution.create!(job_id:, error: 'boom')
    refute @bot.pending_limit_check_job?
  end

  test 'enqueue_limit_check_job enqueues the logged live check job at its next check time' do
    @bot.log_activity('limit_paused', details: { limit_type: :price })
    freeze_time do
      job_setter = mock
      job_setter.expects(:perform_later).with(@bot)
      Bot::PriceLimitCheckJob.expects(:set).with(wait_until: Time.now.utc.end_of_minute).returns(job_setter)
      @bot.enqueue_limit_check_job
    end
  end

  test 'enqueue_limit_check_job is a no-op when there is no limit_paused log' do
    Bot::PriceLimitCheckJob.expects(:set).never
    @bot.enqueue_limit_check_job
  end
end
