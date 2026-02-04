require "test_helper"

# Shared behavior for scheduling cycle tests
module SchedulingCycleBehaviorTests
  extend ActiveSupport::Concern
  include ActiveJob::TestHelper
  include ActiveSupport::Testing::TimeHelpers

  # Subclasses must define #create_bot returning a started bot

  included do
    setup do
      SolidQueue::Job.destroy_all
      SolidQueue::ScheduledExecution.destroy_all
    end

    test "executes bot and schedules next job" do
      bot = create_bot
      setup_cycle_mocks(bot)

      Bot::ActionJob.perform_now(bot)

      assert bot.reload.next_action_job_at.present?
      assert_equal "scheduled", bot.status
    end

    test "schedules next job at correct interval" do
      bot = create_bot
      setup_cycle_mocks(bot)

      freeze_time do
        expected_checkpoint = bot.next_interval_checkpoint_at

        Bot::ActionJob.perform_now(bot)

        actual_scheduled_at = bot.reload.next_action_job_at
        assert_in_delta expected_checkpoint.to_f, actual_scheduled_at.to_f, 1.0
      end
    end

    test "counter starts from bot started_at" do
      bot = create_bot
      setup_cycle_mocks(bot)

      started = bot.started_at
      interval = bot.effective_interval_duration

      next_checkpoint = bot.next_interval_checkpoint_at
      intervals_since_start = ((next_checkpoint - started) / interval).round

      assert intervals_since_start >= 1
    end
  end

  private

  def setup_cycle_mocks(bot)
    setup_bot_execution_mocks(bot)
    bot.stubs(:broadcast_below_minimums_warning)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
  end
end

class BotSchedulingCycleWithSingleAssetTest < ActiveSupport::TestCase
  include SchedulingCycleBehaviorTests

  test "hourly interval schedules next job within an hour" do
    bot = create(:dca_single_asset, :started, :hourly)
    setup_cycle_mocks(bot)

    freeze_time do
      Bot::ActionJob.perform_now(bot)

      next_at = bot.reload.next_action_job_at
      assert next_at <= 1.hour.from_now
    end
  end

  test "weekly interval schedules next job within a week" do
    bot = create(:dca_single_asset, :started, :weekly)
    setup_cycle_mocks(bot)

    freeze_time do
      Bot::ActionJob.perform_now(bot)

      next_at = bot.reload.next_action_job_at
      assert next_at <= 1.week.from_now
    end
  end

  test "below minimum skips transaction but still schedules next" do
    bot = build(:dca_single_asset, :started)
    bot.settings = bot.settings.merge("quote_amount" => 5.0)
    bot.set_missed_quote_amount
    bot.save!

    setup_cycle_mocks(bot)

    Bot::ActionJob.perform_now(bot)

    assert_equal 1, bot.transactions.skipped.count
    assert bot.reload.next_action_job_at.present?
    assert_equal "scheduled", bot.status
  end

  private

  def create_bot
    create(:dca_single_asset, :started)
  end
end

class BotSchedulingCycleWithDualAssetTest < ActiveSupport::TestCase
  include SchedulingCycleBehaviorTests

  private

  def create_bot
    create(:dca_dual_asset, :started)
  end
end

class BotSchedulingCycleRecoveryTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper
  include ActiveSupport::Testing::TimeHelpers

  setup do
    SolidQueue::Job.destroy_all
    SolidQueue::ScheduledExecution.destroy_all
  end

  test "recovers full cycle after system restart" do
    bot = create(:dca_single_asset, :started)
    setup_bot_execution_mocks(bot)
    bot.stubs(:broadcast_below_minimums_warning)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    # Normal operation: execute and schedule
    Bot::ActionJob.perform_now(bot)
    assert bot.reload.next_action_job_at.present?

    # Simulate system restart - jobs lost
    SolidQueue::Job.destroy_all
    SolidQueue::ScheduledExecution.destroy_all
    assert_nil bot.reload.next_action_job_at

    # Run repair job
    Bot::RepairOrphanedBotsJob.perform_now

    # Bot should be rescheduled
    assert bot.reload.next_action_job_at.present?
  end

  test "reschedules after recoverable error" do
    bot = create(:dca_single_asset, :started)
    bot.stubs(:broadcast_below_minimums_warning)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    bot.stubs(:execute_action).returns(Result::Failure.new("Temporary error"))
    bot.stubs(:notify_about_error)

    assert_raises(RuntimeError, "Temporary error") do
      Bot::ActionJob.perform_now(bot)
    end

    assert_equal "retrying", bot.reload.status

    # Repair job should detect and reschedule
    Bot::RepairOrphanedBotsJob.perform_now

    assert bot.reload.next_action_job_at.present?
  end
end
