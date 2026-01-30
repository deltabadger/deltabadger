require 'rails_helper'

RSpec.describe Bot::ActionJob, type: :job do
  include ActiveJob::TestHelper
  include ActiveSupport::Testing::TimeHelpers

  describe '#perform' do
    shared_examples 'action job behavior' do |bot_factory|
      let(:bot) { create(bot_factory, :started) }

      before do
        setup_bot_execution_mocks(bot)
        allow(bot).to receive(:broadcast_below_minimums_warning)
        # Prevent actual job scheduling during tests
        allow(Bot::ActionJob).to receive(:set).and_return(double(perform_later: true))
        allow(Bot::BroadcastAfterScheduledActionJob).to receive(:perform_later)
      end

      context 'when bot is scheduled' do
        it 'executes the bot action' do
          expect(bot).to receive(:execute_action).and_call_original
          described_class.new.perform(bot)
        end

        it 'updates last_action_job_at' do
          freeze_time do
            described_class.new.perform(bot)
            expect(bot.reload.last_action_job_at).to eq(Time.current)
          end
        end

        it 'sets bot status to scheduled after execution' do
          described_class.new.perform(bot)
          expect(bot.reload.status).to eq('scheduled')
        end

        it 'schedules next action job at next_interval_checkpoint_at' do
          job_setter = double(perform_later: true)
          expect(Bot::ActionJob).to receive(:set)
            .with(wait_until: bot.next_interval_checkpoint_at)
            .and_return(job_setter)
          expect(job_setter).to receive(:perform_later).with(bot)

          described_class.new.perform(bot)
        end

        it 'broadcasts after scheduling' do
          expect(Bot::BroadcastAfterScheduledActionJob).to receive(:perform_later).with(bot)
          described_class.new.perform(bot)
        end
      end

      context 'when bot is retrying' do
        before { bot.update!(status: :retrying) }

        it 'executes the bot action' do
          expect(bot).to receive(:execute_action).and_call_original
          described_class.new.perform(bot)
        end
      end

      context 'when bot is not scheduled or retrying' do
        before { bot.update!(status: :stopped) }

        it 'does not execute action' do
          expect(bot).not_to receive(:execute_action)
          described_class.new.perform(bot)
        end
      end

      context 'when bot already has a scheduled action job' do
        before do
          allow(bot).to receive(:next_action_job_at).and_return(1.hour.from_now)
        end

        it 'raises an error' do
          expect { described_class.new.perform(bot) }
            .to raise_error(/already has an action job scheduled/)
        end
      end

      context 'when execute_action fails' do
        before do
          allow(bot).to receive(:execute_action).and_return(Result::Failure.new('Test error'))
          allow(bot).to receive(:notify_about_error)
        end

        it 'raises the error' do
          expect { described_class.new.perform(bot) }
            .to raise_error('Test error')
        end

        it 'sets bot status to retrying' do
          begin
            described_class.new.perform(bot)
          rescue StandardError
            nil
          end
          expect(bot.reload.status).to eq('retrying')
        end
      end

      context 'when execute_action returns break_reschedule' do
        before do
          allow(bot).to receive(:execute_action)
            .and_return(Result::Success.new(break_reschedule: true))
        end

        it 'does not schedule next action job' do
          expect(Bot::ActionJob).not_to receive(:set)
          described_class.new.perform(bot)
        end

        it 'does not update bot status to scheduled' do
          original_status = bot.status
          described_class.new.perform(bot)
          # Status remains unchanged when break_reschedule is true
          expect(bot.reload.status).to eq(original_status)
        end
      end
    end

    context 'with DcaSingleAsset bot' do
      include_examples 'action job behavior', :dca_single_asset
    end

    context 'with DcaDualAsset bot' do
      include_examples 'action job behavior', :dca_dual_asset
    end
  end

  describe 'queue' do
    let(:bot) { create(:dca_single_asset, :started) }

    it 'uses the exchange-specific queue' do
      job = described_class.new(bot)
      # BotJob sets queue based on exchange name_id
      expect(job.queue_name).to eq(bot.exchange.name_id.to_sym)
    end
  end

  describe 'scheduling integration' do
    let(:bot) { create(:dca_single_asset, :started) }

    before do
      setup_bot_execution_mocks(bot)
      allow(bot).to receive(:broadcast_below_minimums_warning)
      allow(Bot::BroadcastAfterScheduledActionJob).to receive(:perform_later)
    end

    it 'creates a scheduled job in SolidQueue' do
      freeze_time do
        # Clear any existing jobs
        SolidQueue::Job.destroy_all

        described_class.new.perform(bot)

        # Check that a job was scheduled
        scheduled_job = SolidQueue::Job.find_by(class_name: 'Bot::ActionJob')
        expect(scheduled_job).to be_present
      end
    end
  end
end
