require 'test_helper'

class Bot::RepairOrphanedBotsJobTest < ActiveSupport::TestCase
  setup do
    # These unit tests stub the scheduled/retrying scope. The job now also runs a second
    # :waiting-recovery pass (Bot.where(status: :waiting).where(updated_at: ..)). Stub that
    # chained relation to return [] so the new pass is an inert no-op here; the real
    # :waiting-recovery behaviour is exercised by the integration class below.
    Bot.stubs(:where).with(status: :waiting).returns(stub(where: []))
  end

  test 'does nothing when there are no orphaned bots' do
    Bot.stubs(:where).returns(Bot.none)
    Rails.logger.expects(:info).with(regexp_matches(/Found.*orphaned bot/)).never

    Bot::RepairOrphanedBotsJob.new.perform
  end

  test 'finds and repairs an orphaned bot' do
    exchange = stub('Exchanges::Binance', present?: true)
    bot = stub(
      'Bots::DcaSingleAsset',
      id: 1,
      class: Bots::DcaSingleAsset,
      exchange: exchange,
      next_action_job_at: nil,
      next_interval_checkpoint_at: 1.hour.from_now,
      cancel_scheduled_action_jobs: true
    )
    job_setter = stub('ConfiguredJob')

    Bot.stubs(:where).with(status: %i[scheduled retrying]).returns([bot])
    Bot::ActionJob.expects(:set).with(wait_until: bot.next_interval_checkpoint_at).returns(job_setter)
    job_setter.expects(:perform_later).with(bot)
    Bot::BroadcastAfterScheduledActionJob.expects(:perform_later).with(bot)

    Bot::RepairOrphanedBotsJob.new.perform
  end

  test 'logs the repair' do
    exchange = stub('Exchanges::Binance', present?: true)
    bot = stub(
      'Bots::DcaSingleAsset',
      id: 1,
      class: Bots::DcaSingleAsset,
      exchange: exchange,
      next_action_job_at: nil,
      next_interval_checkpoint_at: 1.hour.from_now,
      cancel_scheduled_action_jobs: true
    )
    job_setter = stub('ConfiguredJob', perform_later: true)

    Bot.stubs(:where).with(status: %i[scheduled retrying]).returns([bot])
    Bot::ActionJob.stubs(:set).returns(job_setter)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    Rails.logger.expects(:info).with(regexp_matches(/Found 1 orphaned bot/))
    Rails.logger.expects(:warn).with(regexp_matches(/Repairing orphaned bot #{bot.id}/))
    Rails.logger.expects(:info).with(regexp_matches(/Bot #{bot.id} rescheduled/))

    Bot::RepairOrphanedBotsJob.new.perform
  end

  test 'cancels existing jobs before rescheduling' do
    exchange = stub('Exchanges::Binance', present?: true)
    bot = stub(
      'Bots::DcaSingleAsset',
      id: 1,
      class: Bots::DcaSingleAsset,
      exchange: exchange,
      next_action_job_at: nil,
      next_interval_checkpoint_at: 1.hour.from_now
    )
    job_setter = stub('ConfiguredJob', perform_later: true)

    Bot.stubs(:where).with(status: %i[scheduled retrying]).returns([bot])
    Bot::ActionJob.stubs(:set).returns(job_setter)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
    bot.expects(:cancel_scheduled_action_jobs)

    Bot::RepairOrphanedBotsJob.new.perform
  end

  test 'does not repair bot that has a scheduled job' do
    exchange = stub('Exchanges::Binance', present?: true)
    bot = stub(
      'Bots::DcaSingleAsset',
      id: 1,
      exchange: exchange,
      next_action_job_at: 1.hour.from_now
    )

    Bot.stubs(:where).with(status: %i[scheduled retrying]).returns([bot])
    Bot::ActionJob.expects(:set).never

    Bot::RepairOrphanedBotsJob.new.perform
  end

  test 'does not consider bot with no exchange as orphaned' do
    bot = stub(
      'Bots::DcaSingleAsset',
      id: 1,
      exchange: nil
    )

    Bot.stubs(:where).with(status: %i[scheduled retrying]).returns([bot])
    Bot::ActionJob.expects(:set).never

    Bot::RepairOrphanedBotsJob.new.perform
  end

  test 'continues repairing other bots after one fails' do
    exchange = stub('Exchanges::Binance', present?: true)
    bot1 = stub(
      'Bots::DcaSingleAsset',
      id: 1,
      class: Bots::DcaSingleAsset,
      exchange: exchange,
      next_action_job_at: nil,
      next_interval_checkpoint_at: 1.hour.from_now
    )
    bot2 = stub(
      'Bots::DcaSingleAsset',
      id: 2,
      class: Bots::DcaSingleAsset,
      exchange: exchange,
      next_action_job_at: nil,
      next_interval_checkpoint_at: 2.hours.from_now,
      cancel_scheduled_action_jobs: true
    )
    job_setter = stub('ConfiguredJob', perform_later: true)

    Bot.stubs(:where).with(status: %i[scheduled retrying]).returns([bot1, bot2])
    bot1.stubs(:cancel_scheduled_action_jobs).raises(StandardError.new('Test error'))
    Bot::ActionJob.stubs(:set).returns(job_setter)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
    Rails.logger.stubs(:info)
    Rails.logger.stubs(:warn)

    Rails.logger.expects(:error).with(regexp_matches(/Failed to repair bot #{bot1.id}/)).once
    bot2.expects(:cancel_scheduled_action_jobs)

    Bot::RepairOrphanedBotsJob.new.perform
  end

  test 'uses the low_priority queue' do
    assert_equal 'low_priority', Bot::RepairOrphanedBotsJob.new.queue_name
  end
end

# Integration tests with real database records
class Bot::RepairOrphanedBotsJobIntegrationTest < ActiveSupport::TestCase
  setup do
    SolidQueue::Job.destroy_all
    SolidQueue::ScheduledExecution.destroy_all
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
  end

  # == Single asset integration tests ==

  test 'detects orphaned single asset bot with no scheduled job' do
    bot = create(:dca_single_asset, :started, status: :scheduled)
    assert_nil bot.next_action_job_at

    Bot::RepairOrphanedBotsJob.perform_now

    assert bot.reload.next_action_job_at.present?
  end

  test 'schedules single asset bot job at correct checkpoint time' do
    bot = create(:dca_single_asset, :started, status: :scheduled)
    expected_checkpoint = bot.next_interval_checkpoint_at

    Bot::RepairOrphanedBotsJob.perform_now

    scheduled_at = bot.reload.next_action_job_at
    assert_in_delta expected_checkpoint.to_f, scheduled_at.to_f, 1.0
  end

  test 'creates ActionJob in SolidQueue for single asset bot' do
    create(:dca_single_asset, :started, status: :scheduled)

    Bot::RepairOrphanedBotsJob.perform_now

    job = SolidQueue::Job.find_by(class_name: 'Bot::ActionJob')
    assert job.present?
  end

  test 'does not repair single asset bot that already has a scheduled job' do
    bot = create(:dca_single_asset, :started, status: :scheduled)
    Bot::ActionJob.set(wait_until: 1.hour.from_now).perform_later(bot)
    initial_job_count = SolidQueue::Job.where(class_name: 'Bot::ActionJob').count

    Bot::RepairOrphanedBotsJob.perform_now

    assert_equal initial_job_count, SolidQueue::Job.where(class_name: 'Bot::ActionJob').count
  end

  # == Dual asset integration tests ==

  test 'detects orphaned dual asset bot with no scheduled job' do
    bot = create(:dca_dual_asset, :started, status: :scheduled)
    assert_nil bot.next_action_job_at

    Bot::RepairOrphanedBotsJob.perform_now

    assert bot.reload.next_action_job_at.present?
  end

  test 'schedules dual asset bot job at correct checkpoint time' do
    bot = create(:dca_dual_asset, :started, status: :scheduled)
    expected_checkpoint = bot.next_interval_checkpoint_at

    Bot::RepairOrphanedBotsJob.perform_now

    scheduled_at = bot.reload.next_action_job_at
    assert_in_delta expected_checkpoint.to_f, scheduled_at.to_f, 1.0
  end

  test 'creates ActionJob in SolidQueue for dual asset bot' do
    create(:dca_dual_asset, :started, status: :scheduled)

    Bot::RepairOrphanedBotsJob.perform_now

    job = SolidQueue::Job.find_by(class_name: 'Bot::ActionJob')
    assert job.present?
  end

  test 'does not repair dual asset bot that already has a scheduled job' do
    bot = create(:dca_dual_asset, :started, status: :scheduled)
    Bot::ActionJob.set(wait_until: 1.hour.from_now).perform_later(bot)
    initial_job_count = SolidQueue::Job.where(class_name: 'Bot::ActionJob').count

    Bot::RepairOrphanedBotsJob.perform_now

    assert_equal initial_job_count, SolidQueue::Job.where(class_name: 'Bot::ActionJob').count
  end

  # == Mixed bot type tests ==

  test 'repairs all orphaned bots of different types' do
    exchange = create(:binance_exchange)
    bitcoin = create(:asset, :bitcoin)
    ethereum = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    single_asset_bot = create(:dca_single_asset, :started, status: :scheduled,
                                                           exchange: exchange, base_asset: bitcoin, quote_asset: usd)
    dual_asset_bot = create(:dca_dual_asset, :started, status: :scheduled,
                                                       exchange: exchange, base0_asset: bitcoin, base1_asset: ethereum, quote_asset: usd)

    assert_nil single_asset_bot.next_action_job_at
    assert_nil dual_asset_bot.next_action_job_at

    Bot::RepairOrphanedBotsJob.perform_now

    assert single_asset_bot.reload.next_action_job_at.present?
    assert dual_asset_bot.reload.next_action_job_at.present?
  end

  test 'creates separate jobs for each bot' do
    exchange = create(:binance_exchange)
    bitcoin = create(:asset, :bitcoin)
    ethereum = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    create(:dca_single_asset, :started, status: :scheduled,
                                        exchange: exchange, base_asset: bitcoin, quote_asset: usd)
    create(:dca_dual_asset, :started, status: :scheduled,
                                      exchange: exchange, base0_asset: bitcoin, base1_asset: ethereum, quote_asset: usd)

    Bot::RepairOrphanedBotsJob.perform_now

    jobs = SolidQueue::Job.where(class_name: 'Bot::ActionJob')
    assert_equal 2, jobs.count
  end

  test 'repairs retrying bot with no scheduled job' do
    bot = create(:dca_single_asset, :started, status: :retrying)
    assert_nil bot.next_action_job_at

    Bot::RepairOrphanedBotsJob.perform_now

    assert bot.reload.next_action_job_at.present?
  end

  test 'does not repair stopped bots' do
    create(:dca_single_asset, :stopped)

    Bot::RepairOrphanedBotsJob.perform_now

    assert_equal 0, SolidQueue::Job.where(class_name: 'Bot::ActionJob').count
  end

  test 'recovers bot scheduling after simulated restart' do
    bot = create(:dca_single_asset, :started, status: :scheduled)

    Bot::ActionJob.set(wait_until: bot.next_interval_checkpoint_at).perform_later(bot)
    assert bot.next_action_job_at.present?

    SolidQueue::Job.destroy_all
    SolidQueue::ScheduledExecution.destroy_all
    assert_nil bot.reload.next_action_job_at

    Bot::RepairOrphanedBotsJob.perform_now

    assert bot.reload.next_action_job_at.present?
  end
end

class Bot::RepairOrphanedWaitingBotsJobIntegrationTest < ActiveSupport::TestCase
  setup do
    SolidQueue::Job.destroy_all
    SolidQueue::ScheduledExecution.destroy_all
    SolidQueue::ReadyExecution.destroy_all
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
  end

  # A :waiting limit-paused bot whose check chain died (limit_paused logged, no pending job),
  # STABLY waiting past the WEDGE_GRACE window (backdate updated_at so the staleness guard matches).
  def waiting_paused_bot(limit_type: :price, stale: true, **limit_attrs)
    bot = create(:dca_single_asset, :started)
    # limit_attrs change `settings`, which the Accountable invariant guards — set
    # missed_quote_amount before saving (mirrors the factory's before(:create) hook).
    limit_attrs.each { |k, v| bot.public_send("#{k}=", v) }
    bot.status = :waiting
    bot.last_action_job_at = Time.current # transient_data, not settings
    bot.set_missed_quote_amount
    bot.save!
    bot.log_activity('limit_paused', details: { limit_type: limit_type }) # pause AFTER last run → current
    bot.update_column(:updated_at, 5.minutes.ago) if stale # past WEDGE_GRACE (2 min)
    bot
  end

  test 'recovers a :waiting price-limited bot whose check chain has died' do
    bot = waiting_paused_bot(limit_type: :price, price_limited: true)
    refute bot.pending_limit_check_job?, 'precondition: no pending check job'

    Bot::RepairOrphanedBotsJob.perform_now

    assert bot.reload.pending_limit_check_job?, 'a PriceLimitCheckJob should be re-enqueued'
    assert_equal 'waiting', bot.status, 'recovery must NOT change the bot status'
    assert_equal 1, SolidQueue::Job.where(class_name: 'Bot::PriceLimitCheckJob').count
  end

  # Codex R2 #1: a bot that limit-paused in the PAST keeps its limit_paused log forever; a momentary
  # normal :waiting (just flipped, updated_at recent) must NOT be misread as wedged.
  test 'does NOT sweep a bot only momentarily :waiting (within the grace window)' do
    waiting_paused_bot(limit_type: :price, price_limited: true, stale: false) # updated_at = now

    Bot::RepairOrphanedBotsJob.perform_now

    assert_equal 0, SolidQueue::Job.where(class_name: 'Bot::PriceLimitCheckJob').count,
                 'a freshly-:waiting bot is still churning — do not recover yet'
  end

  test 'does NOT touch a :waiting bot that still has a queued check job' do
    bot = waiting_paused_bot(limit_type: :price, price_limited: true)
    Bot::PriceLimitCheckJob.set(wait_until: 1.minute.from_now).perform_later(bot)
    initial = SolidQueue::Job.where(class_name: 'Bot::PriceLimitCheckJob').count

    Bot::RepairOrphanedBotsJob.perform_now

    assert_equal initial, SolidQueue::Job.where(class_name: 'Bot::PriceLimitCheckJob').count,
                 'must not double-enqueue a still-pending chain'
  end

  test 'does NOT recover a :waiting bot with no limit_paused log (not limit-paused)' do
    bot = create(:dca_single_asset, :started)
    bot.update!(status: :waiting) # waiting but never limit-paused (edge case)
    bot.update_column(:updated_at, 5.minutes.ago)

    Bot::RepairOrphanedBotsJob.perform_now

    assert_equal 0, SolidQueue::Job.where('class_name LIKE ?', '%LimitCheckJob').count
  end

  # Codex High #1: indicator enabled AND satisfied, price enabled AND unmet → the decorator that
  # paused was PRICE (it logged limit_type: :price). Recovery must re-enqueue PriceLimitCheckJob,
  # NOT IndicatorLimitCheckJob — proving we follow the limit_paused log, not enabled-predicate order.
  test 'recovers with the job of the limit that ACTUALLY paused, not the outermost enabled one' do
    waiting_paused_bot(limit_type: :price, price_limited: true, indicator_limited: true)

    Bot::RepairOrphanedBotsJob.perform_now

    assert_equal 1, SolidQueue::Job.where(class_name: 'Bot::PriceLimitCheckJob').count
    assert_equal 0, SolidQueue::Job.where(class_name: 'Bot::IndicatorLimitCheckJob').count
  end
end
