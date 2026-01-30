require 'rails_helper'

RSpec.describe Bot::RepairOrphanedBotsJob, type: :job do
  describe '#perform' do
    context 'when there are no orphaned bots' do
      before do
        allow(Bot).to receive(:where).and_return(Bot.none)
      end

      it 'does nothing' do
        expect(Rails.logger).not_to receive(:info).with(/Found.*orphaned bot/)
        described_class.new.perform
      end
    end

    context 'when there is an orphaned bot' do
      let(:exchange) { instance_double('Exchanges::Binance', present?: true) }
      let(:bot) do
        instance_double(
          'Bots::DcaSingleAsset',
          id: 1,
          class: Bots::DcaSingleAsset,
          exchange: exchange,
          next_action_job_at: nil,
          next_interval_checkpoint_at: 1.hour.from_now,
          cancel_scheduled_action_jobs: true
        )
      end
      let(:job_setter) { instance_double(ActiveJob::ConfiguredJob) }

      before do
        allow(Bot).to receive(:where).with(status: [:scheduled, :retrying]).and_return([bot])
      end

      it 'finds and repairs the orphaned bot' do
        expect(Bot::ActionJob).to receive(:set).with(wait_until: bot.next_interval_checkpoint_at).and_return(job_setter)
        expect(job_setter).to receive(:perform_later).with(bot)
        expect(Bot::BroadcastAfterScheduledActionJob).to receive(:perform_later).with(bot)

        described_class.new.perform
      end

      it 'logs the repair' do
        allow(Bot::ActionJob).to receive(:set).and_return(job_setter)
        allow(job_setter).to receive(:perform_later)
        allow(Bot::BroadcastAfterScheduledActionJob).to receive(:perform_later)

        expect(Rails.logger).to receive(:info).with(/Found 1 orphaned bot/)
        expect(Rails.logger).to receive(:warn).with(/Repairing orphaned bot #{bot.id}/)
        expect(Rails.logger).to receive(:info).with(/Bot #{bot.id} rescheduled/)

        described_class.new.perform
      end

      it 'cancels any existing jobs before rescheduling' do
        allow(Bot::ActionJob).to receive(:set).and_return(job_setter)
        allow(job_setter).to receive(:perform_later)
        allow(Bot::BroadcastAfterScheduledActionJob).to receive(:perform_later)

        expect(bot).to receive(:cancel_scheduled_action_jobs)

        described_class.new.perform
      end
    end

    context 'when bot has a scheduled job (not orphaned)' do
      let(:exchange) { instance_double('Exchanges::Binance', present?: true) }
      let(:bot) do
        instance_double(
          'Bots::DcaSingleAsset',
          id: 1,
          exchange: exchange,
          next_action_job_at: 1.hour.from_now
        )
      end

      before do
        allow(Bot).to receive(:where).with(status: [:scheduled, :retrying]).and_return([bot])
      end

      it 'does not repair the bot' do
        expect(Bot::ActionJob).not_to receive(:set)
        described_class.new.perform
      end
    end

    context 'when bot has no exchange' do
      let(:bot) do
        instance_double(
          'Bots::DcaSingleAsset',
          id: 1,
          exchange: nil
        )
      end

      before do
        allow(Bot).to receive(:where).with(status: [:scheduled, :retrying]).and_return([bot])
      end

      it 'does not consider the bot as orphaned' do
        expect(Bot::ActionJob).not_to receive(:set)
        described_class.new.perform
      end
    end

    context 'when repair fails for one bot' do
      let(:exchange) { instance_double('Exchanges::Binance', present?: true) }
      let(:bot1) do
        instance_double(
          'Bots::DcaSingleAsset',
          id: 1,
          class: Bots::DcaSingleAsset,
          exchange: exchange,
          next_action_job_at: nil,
          next_interval_checkpoint_at: 1.hour.from_now
        )
      end
      let(:bot2) do
        instance_double(
          'Bots::DcaSingleAsset',
          id: 2,
          class: Bots::DcaSingleAsset,
          exchange: exchange,
          next_action_job_at: nil,
          next_interval_checkpoint_at: 2.hours.from_now,
          cancel_scheduled_action_jobs: true
        )
      end
      let(:job_setter) { instance_double(ActiveJob::ConfiguredJob) }

      before do
        allow(Bot).to receive(:where).with(status: [:scheduled, :retrying]).and_return([bot1, bot2])
        allow(bot1).to receive(:cancel_scheduled_action_jobs).and_raise(StandardError.new('Test error'))
      end

      it 'continues repairing other bots after one fails' do
        allow(Bot::ActionJob).to receive(:set).and_return(job_setter)
        allow(job_setter).to receive(:perform_later)
        allow(Bot::BroadcastAfterScheduledActionJob).to receive(:perform_later)

        # Use allow for messages we don't care about order, expect for ones we do
        allow(Rails.logger).to receive(:info)
        allow(Rails.logger).to receive(:warn)

        expect(Rails.logger).to receive(:error).with(/Failed to repair bot #{bot1.id}/).once
        expect(bot2).to receive(:cancel_scheduled_action_jobs)

        described_class.new.perform
      end
    end
  end

  describe 'queue' do
    it 'uses the low_priority queue' do
      expect(described_class.new.queue_name).to eq('low_priority')
    end
  end

  describe 'integration tests' do
    # These tests use real database records and SolidQueue to verify
    # the full flow of detecting and repairing orphaned bots after
    # a system restart (e.g., computer turned off)

    before do
      # Clear all jobs to simulate a fresh start after system restart
      SolidQueue::Job.destroy_all
      SolidQueue::ScheduledExecution.destroy_all
      allow(Bot::BroadcastAfterScheduledActionJob).to receive(:perform_later)
    end

    shared_examples 'repairs orphaned bot' do |bot_factory|
      context "with #{bot_factory} bot" do
        let!(:bot) { create(bot_factory, :started, status: :scheduled) }

        it 'detects orphaned bot with no scheduled job' do
          # Bot is scheduled but has no job in queue (simulates restart)
          expect(bot.next_action_job_at).to be_nil

          described_class.perform_now

          # After repair, bot should have a scheduled job
          expect(bot.reload.next_action_job_at).to be_present
        end

        it 'schedules job at correct checkpoint time' do
          expected_checkpoint = bot.next_interval_checkpoint_at

          described_class.perform_now

          scheduled_at = bot.reload.next_action_job_at
          expect(scheduled_at).to be_within(1.second).of(expected_checkpoint)
        end

        it 'creates ActionJob in SolidQueue' do
          described_class.perform_now

          job = SolidQueue::Job.find_by(class_name: 'Bot::ActionJob')
          expect(job).to be_present
        end

        it 'does not repair bot that already has a scheduled job' do
          # Schedule a job for the bot
          Bot::ActionJob.set(wait_until: 1.hour.from_now).perform_later(bot)
          initial_job_count = SolidQueue::Job.where(class_name: 'Bot::ActionJob').count

          described_class.perform_now

          # Should not create additional jobs
          expect(SolidQueue::Job.where(class_name: 'Bot::ActionJob').count).to eq(initial_job_count)
        end
      end
    end

    include_examples 'repairs orphaned bot', :dca_single_asset
    include_examples 'repairs orphaned bot', :dca_dual_asset

    context 'with multiple orphaned bots of different types' do
      # Create shared resources to avoid uniqueness conflicts
      let(:exchange) { create(:binance_exchange) }
      let(:bitcoin) { create(:asset, :bitcoin) }
      let(:ethereum) { create(:asset, :ethereum) }
      let(:usd) { create(:asset, :usd) }
      let!(:single_asset_bot) do
        create(:dca_single_asset, :started, status: :scheduled,
               exchange: exchange, base_asset: bitcoin, quote_asset: usd)
      end
      let!(:dual_asset_bot) do
        create(:dca_dual_asset, :started, status: :scheduled,
               exchange: exchange, base0_asset: bitcoin, base1_asset: ethereum, quote_asset: usd)
      end

      it 'repairs all orphaned bots' do
        expect(single_asset_bot.next_action_job_at).to be_nil
        expect(dual_asset_bot.next_action_job_at).to be_nil

        described_class.perform_now

        expect(single_asset_bot.reload.next_action_job_at).to be_present
        expect(dual_asset_bot.reload.next_action_job_at).to be_present
      end

      it 'creates separate jobs for each bot' do
        described_class.perform_now

        jobs = SolidQueue::Job.where(class_name: 'Bot::ActionJob')
        expect(jobs.count).to eq(2)
      end
    end

    context 'with bot in retrying status' do
      let!(:bot) { create(:dca_single_asset, :started, status: :retrying) }

      it 'repairs retrying bot with no scheduled job' do
        expect(bot.next_action_job_at).to be_nil

        described_class.perform_now

        expect(bot.reload.next_action_job_at).to be_present
      end
    end

    context 'with stopped bot' do
      let!(:bot) { create(:dca_single_asset, :stopped) }

      it 'does not repair stopped bots' do
        described_class.perform_now

        expect(bot.reload.next_action_job_at).to be_nil
        expect(SolidQueue::Job.where(class_name: 'Bot::ActionJob').count).to eq(0)
      end
    end

    context 'simulating system restart scenario' do
      let!(:bot) { create(:dca_single_asset, :started, status: :scheduled) }

      it 'recovers bot scheduling after simulated restart' do
        # First, schedule a job as if bot was running normally
        Bot::ActionJob.set(wait_until: bot.next_interval_checkpoint_at).perform_later(bot)
        expect(bot.next_action_job_at).to be_present

        # Simulate system restart - all jobs are lost
        SolidQueue::Job.destroy_all
        SolidQueue::ScheduledExecution.destroy_all
        expect(bot.reload.next_action_job_at).to be_nil

        # Run the repair job (as would happen on startup)
        described_class.perform_now

        # Bot should be rescheduled
        expect(bot.reload.next_action_job_at).to be_present
      end
    end
  end
end
