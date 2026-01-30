require 'rails_helper'

RSpec.describe 'Bot Scheduling Cycle', type: :integration do
  # Integration tests for the complete bot scheduling cycle:
  # 1. Counter ends (interval checkpoint reached)
  # 2. Bot executes: performs transaction OR adds to buffer if below minimum
  # 3. Next transaction is scheduled
  # 4. Counter starts counting to the next transaction

  include ActiveJob::TestHelper
  include ActiveSupport::Testing::TimeHelpers

  before do
    SolidQueue::Job.destroy_all
    SolidQueue::ScheduledExecution.destroy_all
  end

  shared_examples 'complete scheduling cycle' do |bot_factory|
    let(:bot) { create(bot_factory, :started) }

    before do
      setup_bot_execution_mocks(bot)
      allow(bot).to receive(:broadcast_below_minimums_warning)
      allow(Bot::BroadcastAfterScheduledActionJob).to receive(:perform_later)
    end

    describe 'normal execution cycle' do
      it 'executes bot and schedules next job' do
        # Execute the action job
        Bot::ActionJob.perform_now(bot)

        # Next job should be scheduled
        expect(bot.reload.next_action_job_at).to be_present

        # Bot should be in scheduled status
        expect(bot.status).to eq('scheduled')
      end

      it 'schedules next job at correct interval' do
        freeze_time do
          expected_checkpoint = bot.next_interval_checkpoint_at

          Bot::ActionJob.perform_now(bot)

          actual_scheduled_at = bot.reload.next_action_job_at
          expect(actual_scheduled_at).to be_within(1.second).of(expected_checkpoint)
        end
      end

      it 'counter starts from bot started_at' do
        # Bot started_at determines the interval checkpoints
        started = bot.started_at
        interval = bot.effective_interval_duration

        # Next checkpoint should be aligned to started_at
        next_checkpoint = bot.next_interval_checkpoint_at
        intervals_since_start = ((next_checkpoint - started) / interval).round

        expect(intervals_since_start).to be >= 1
      end
    end
  end

  context 'with DcaSingleAsset bot' do
    include_examples 'complete scheduling cycle', :dca_single_asset

    describe 'hourly interval' do
      let(:bot) { create(:dca_single_asset, :started, :hourly) }

      before do
        setup_bot_execution_mocks(bot)
        allow(bot).to receive(:broadcast_below_minimums_warning)
        allow(Bot::BroadcastAfterScheduledActionJob).to receive(:perform_later)
      end

      it 'schedules next job within an hour' do
        freeze_time do
          Bot::ActionJob.perform_now(bot)

          next_at = bot.reload.next_action_job_at
          expect(next_at).to be <= 1.hour.from_now
        end
      end
    end

    describe 'weekly interval' do
      let(:bot) { create(:dca_single_asset, :started, :weekly) }

      before do
        setup_bot_execution_mocks(bot)
        allow(bot).to receive(:broadcast_below_minimums_warning)
        allow(Bot::BroadcastAfterScheduledActionJob).to receive(:perform_later)
      end

      it 'schedules next job within a week' do
        freeze_time do
          Bot::ActionJob.perform_now(bot)

          next_at = bot.reload.next_action_job_at
          expect(next_at).to be <= 1.week.from_now
        end
      end
    end

    describe 'below minimum amount handling' do
      let(:bot) do
        # Create bot with small quote_amount (below minimum of 10)
        bot = build(:dca_single_asset, :started)
        bot.settings = bot.settings.merge('quote_amount' => 5.0)
        bot.set_missed_quote_amount
        bot.save!
        bot
      end

      before do
        setup_bot_execution_mocks(bot)
        allow(bot).to receive(:broadcast_below_minimums_warning)
        allow(Bot::BroadcastAfterScheduledActionJob).to receive(:perform_later)
      end

      it 'skips transaction but still schedules next' do
        Bot::ActionJob.perform_now(bot)

        # Transaction should be skipped (below minimum)
        expect(bot.transactions.skipped.count).to eq(1)

        # Next job should still be scheduled
        expect(bot.reload.next_action_job_at).to be_present
        expect(bot.status).to eq('scheduled')
      end
    end
  end

  context 'with DcaDualAsset bot' do
    include_examples 'complete scheduling cycle', :dca_dual_asset
  end

  describe 'system restart recovery cycle' do
    let(:bot) { create(:dca_single_asset, :started) }

    before do
      setup_bot_execution_mocks(bot)
      allow(bot).to receive(:broadcast_below_minimums_warning)
      allow(Bot::BroadcastAfterScheduledActionJob).to receive(:perform_later)
    end

    it 'recovers full cycle after system restart' do
      # Normal operation: execute and schedule
      Bot::ActionJob.perform_now(bot)
      expect(bot.reload.next_action_job_at).to be_present

      # Simulate system restart - jobs lost
      SolidQueue::Job.destroy_all
      SolidQueue::ScheduledExecution.destroy_all
      expect(bot.reload.next_action_job_at).to be_nil

      # Run repair job
      Bot::RepairOrphanedBotsJob.perform_now

      # Bot should be rescheduled
      expect(bot.reload.next_action_job_at).to be_present
    end
  end

  describe 'error recovery cycle' do
    let(:bot) { create(:dca_single_asset, :started) }

    before do
      allow(bot).to receive(:broadcast_below_minimums_warning)
      allow(Bot::BroadcastAfterScheduledActionJob).to receive(:perform_later)
    end

    it 'reschedules after recoverable error' do
      # First call fails
      allow(bot).to receive(:execute_action).and_return(Result::Failure.new('Temporary error'))
      allow(bot).to receive(:notify_about_error)

      expect {
        Bot::ActionJob.perform_now(bot)
      }.to raise_error('Temporary error')

      # Bot should be in retrying status
      expect(bot.reload.status).to eq('retrying')

      # Repair job should detect and reschedule
      Bot::RepairOrphanedBotsJob.perform_now

      expect(bot.reload.next_action_job_at).to be_present
    end
  end
end
