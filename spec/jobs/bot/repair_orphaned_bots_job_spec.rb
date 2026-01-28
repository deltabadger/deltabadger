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
end
