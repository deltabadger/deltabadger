require 'test_helper'

# Shared behavior tests for Bot::ActionJob across both bot types
module ActionJobBehaviorTests
  extend ActiveSupport::Concern
  include ActiveJob::TestHelper
  include ActiveSupport::Testing::TimeHelpers

  # Subclasses must define #create_bot returning a started bot

  included do
    test 'executes the bot action when scheduled' do
      bot = create_bot
      setup_action_job_mocks(bot)
      bot.expects(:execute_action).returns(Result::Success.new)

      Bot::ActionJob.new.perform(bot)
    end

    test 'updates last_action_job_at' do
      bot = create_bot
      setup_action_job_mocks(bot)

      freeze_time do
        Bot::ActionJob.new.perform(bot)
        assert_equal Time.current, bot.reload.last_action_job_at
      end
    end

    test 'sets bot status to scheduled after execution' do
      bot = create_bot
      setup_action_job_mocks(bot)

      Bot::ActionJob.new.perform(bot)
      assert_equal 'scheduled', bot.reload.status
    end

    test 'schedules next action job at next_interval_checkpoint_at' do
      bot = create_bot
      setup_action_job_mocks(bot)

      job_setter = stub(perform_later: true)
      Bot::ActionJob.unstub(:set)
      Bot::ActionJob.expects(:set)
                    .with(wait_until: bot.next_interval_checkpoint_at)
                    .returns(job_setter)
      job_setter.expects(:perform_later).with(bot)

      Bot::ActionJob.new.perform(bot)
    end

    test 'broadcasts after scheduling' do
      bot = create_bot
      setup_action_job_mocks(bot)
      Bot::BroadcastAfterScheduledActionJob.expects(:perform_later).with(bot)

      Bot::ActionJob.new.perform(bot)
    end

    test 'executes action when bot is retrying' do
      bot = create_bot
      bot.update!(status: :retrying)
      setup_action_job_mocks(bot)
      bot.expects(:execute_action).returns(Result::Success.new)

      Bot::ActionJob.new.perform(bot)
    end

    test 'does not execute action when bot is stopped' do
      bot = create_bot
      bot.update!(status: :stopped)
      setup_action_job_mocks(bot)
      bot.expects(:execute_action).never

      Bot::ActionJob.new.perform(bot)
    end

    test 'raises error when bot already has a scheduled action job' do
      bot = create_bot
      setup_action_job_mocks(bot)
      bot.stubs(:next_action_job_at).returns(1.hour.from_now)

      error = assert_raises(RuntimeError) do
        Bot::ActionJob.new.perform(bot)
      end
      assert_match(/already has an action job scheduled/, error.message)
    end

    test 'raises error when execute_action fails' do
      bot = create_bot
      setup_action_job_mocks(bot)
      bot.stubs(:execute_action).returns(Result::Failure.new('Test error'))
      bot.stubs(:notify_about_error)

      assert_raises(RuntimeError, 'Test error') do
        Bot::ActionJob.new.perform(bot)
      end
    end

    test 'sets bot status to retrying when execute_action fails' do
      bot = create_bot
      setup_action_job_mocks(bot)
      bot.stubs(:execute_action).returns(Result::Failure.new('Test error'))
      bot.stubs(:notify_about_error)

      begin
        Bot::ActionJob.new.perform(bot)
      rescue StandardError
        nil
      end
      assert_equal 'retrying', bot.reload.status
    end

    test 'does not schedule next job when break_reschedule is true' do
      bot = create_bot
      setup_action_job_mocks(bot)
      bot.stubs(:execute_action).returns(Result::Success.new(break_reschedule: true))
      Bot::ActionJob.expects(:set).never

      Bot::ActionJob.new.perform(bot)
    end

    test 'does not update status when break_reschedule is true' do
      bot = create_bot
      setup_action_job_mocks(bot)
      original_status = bot.status
      bot.stubs(:execute_action).returns(Result::Success.new(break_reschedule: true))

      Bot::ActionJob.new.perform(bot)
      assert_equal original_status, bot.reload.status
    end
  end

  private

  def setup_action_job_mocks(bot)
    setup_bot_execution_mocks(bot)
    bot.stubs(:broadcast_below_minimums_warning)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
  end
end

class Bot::ActionJobWithSingleAssetTest < ActiveSupport::TestCase
  include ActionJobBehaviorTests

  private

  def create_bot
    create(:dca_single_asset, :started)
  end
end

class Bot::ActionJobWithDualAssetTest < ActiveSupport::TestCase
  include ActionJobBehaviorTests

  private

  def create_bot
    create(:dca_dual_asset, :started)
  end
end

class Bot::ActionJobQueueTest < ActiveSupport::TestCase
  test 'uses the exchange-specific queue' do
    bot = create(:dca_single_asset, :started)
    job = Bot::ActionJob.new(bot)
    assert_equal bot.exchange.name_id.to_sym, job.queue_name
  end
end

class Bot::ActionJobSchedulingIntegrationTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

  test 'creates a scheduled job in SolidQueue' do
    bot = create(:dca_single_asset, :started)
    setup_bot_execution_mocks(bot)
    bot.stubs(:broadcast_below_minimums_warning)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    freeze_time do
      SolidQueue::Job.destroy_all

      Bot::ActionJob.new.perform(bot)

      scheduled_job = SolidQueue::Job.find_by(class_name: 'Bot::ActionJob')
      assert scheduled_job.present?
    end
  end
end
