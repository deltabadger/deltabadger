require 'test_helper'

class Bot::RepairOrphanedBotsJobTest < ActiveSupport::TestCase
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
