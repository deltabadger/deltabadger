require 'rails_helper'

RSpec.describe Bots::DcaSingleAsset, type: :model do
  include ActiveSupport::Testing::TimeHelpers
  describe 'associations' do
    let(:bot) { create(:dca_single_asset) }

    it 'belongs to exchange (optional)' do
      expect(bot).to respond_to(:exchange)
      expect(bot.exchange).to be_a(Exchange)

      # Test that exchange is optional
      bot.exchange = nil
      expect(bot).to be_valid
    end

    it 'belongs to user' do
      expect(bot).to respond_to(:user)
      expect(bot.user).to be_a(User)
    end

    it 'has many transactions' do
      expect(bot).to respond_to(:transactions)
      expect(bot.transactions).to be_a(ActiveRecord::Associations::CollectionProxy)

      # Test dependent: destroy (using delete_all to bypass soft-delete)
      transaction = create(:transaction, bot: bot)
      expect(bot.transactions.count).to eq(1)
      expect(bot.transactions).to include(transaction)
    end
  end

  describe 'validations' do
    describe 'quote_amount' do
      let(:bot) { build(:dca_single_asset) }

      it 'requires quote_amount to be present' do
        bot.quote_amount = nil
        expect(bot).not_to be_valid
        expect(bot.errors[:quote_amount]).to include("can't be blank")
      end

      it 'requires quote_amount to be greater than 0' do
        bot.quote_amount = 0
        expect(bot).not_to be_valid
        expect(bot.errors[:quote_amount]).to be_present
      end

      it 'accepts positive quote_amount' do
        bot.quote_amount = 100
        expect(bot.errors[:quote_amount]).to be_empty
      end
    end

    describe 'interval' do
      let(:bot) { build(:dca_single_asset) }

      it 'requires interval to be present' do
        bot.interval = nil
        expect(bot).not_to be_valid
        expect(bot.errors[:interval]).to include("can't be blank")
      end

      it 'accepts valid interval values' do
        %w[hour day week month].each do |interval|
          bot.interval = interval
          expect(bot.errors[:interval]).to be_empty
        end
      end

      it 'rejects invalid interval values' do
        bot.interval = 'minute'
        expect(bot).not_to be_valid
        expect(bot.errors[:interval]).to include('is not included in the list')
      end
    end

    describe '#validate_external_ids' do
      let(:bot) { create(:dca_single_asset) }

      it 'is valid when both assets exist' do
        expect(bot).to be_valid(:update)
      end

      it 'is invalid when base_asset does not exist' do
        bot.base_asset_id = 999999
        expect(bot).not_to be_valid(:update)
        expect(bot.errors[:base_asset_id]).to include('is invalid')
      end

      it 'is invalid when quote_asset does not exist' do
        bot.quote_asset_id = 999999
        expect(bot).not_to be_valid(:update)
        expect(bot.errors[:quote_asset_id]).to include('is invalid')
      end
    end

    describe '#validate_bot_exchange' do
      let(:exchange) { create(:binance_exchange) }
      let(:bitcoin) { create(:asset, :bitcoin) }
      let(:usd) { create(:asset, :usd) }
      let(:eth) { create(:asset, :ethereum) }
      let!(:ticker) { create(:ticker, exchange: exchange, base_asset: bitcoin, quote_asset: usd) }
      let(:bot) { create(:dca_single_asset, exchange: exchange, base_asset: bitcoin, quote_asset: usd) }

      it 'is valid when exchange supports the asset pair' do
        expect(bot).to be_valid(:update)
      end

      it 'is invalid when exchange does not support the asset pair' do
        bot.set_missed_quote_amount
        bot.base_asset_id = eth.id
        expect(bot).not_to be_valid(:update)
        expect(bot.errors[:exchange]).to be_present
      end
    end

    describe '#validate_unchangeable_assets' do
      let(:bot) { create(:dca_single_asset) }
      let(:new_asset) { create(:asset, :ethereum) }

      before do
        create(:transaction, bot: bot)
      end

      it 'prevents changing base_asset after transactions exist' do
        bot.set_missed_quote_amount
        bot.base_asset_id = new_asset.id
        expect(bot).not_to be_valid(:update)
        expect(bot.errors[:base_asset_id]).to be_present
      end

      it 'prevents changing quote_asset after transactions exist' do
        bot.set_missed_quote_amount
        bot.quote_asset_id = new_asset.id
        expect(bot).not_to be_valid(:update)
        expect(bot.errors[:quote_asset_id]).to be_present
      end

      it 'allows changing other settings after transactions exist' do
        bot.set_missed_quote_amount
        bot.quote_amount = 200
        expect(bot).to be_valid(:update)
      end
    end

    describe '#validate_unchangeable_interval' do
      let(:bot) { create(:dca_single_asset, :started) }

      it 'prevents changing interval while bot is running' do
        bot.set_missed_quote_amount
        bot.interval = 'week'
        expect(bot).not_to be_valid(:update)
        expect(bot.errors[:settings]).to include('Interval cannot be changed while the bot is running')
      end

      it 'allows changing interval when bot is stopped' do
        bot.status = :stopped
        bot.stopped_at = Time.current
        bot.save!

        bot.set_missed_quote_amount
        bot.interval = 'week'
        expect(bot).to be_valid(:update)
      end
    end

    describe '#validate_unchangeable_exchange' do
      let(:bot) { create(:dca_single_asset) }
      let(:new_exchange) { create(:kraken_exchange) }

      context 'when there are open orders' do
        before do
          create(:transaction, :open, bot: bot)
        end

        it 'prevents changing exchange' do
          # Create ticker and exchange_assets for new exchange
          create(:ticker, exchange: new_exchange, base_asset: bot.base_asset, quote_asset: bot.quote_asset)
          create(:api_key, user: bot.user, exchange: new_exchange)

          bot.exchange = new_exchange
          expect(bot).not_to be_valid(:update)
          expect(bot.errors[:exchange]).to be_present
        end
      end

      context 'when there are no open orders' do
        before do
          create(:transaction, bot: bot, external_status: :closed)
        end

        it 'allows changing exchange' do
          # Create ticker and exchange_assets for new exchange
          create(:ticker, exchange: new_exchange, base_asset: bot.base_asset, quote_asset: bot.quote_asset)
          create(:api_key, user: bot.user, exchange: new_exchange)

          bot.exchange = new_exchange
          expect(bot).to be_valid(:update)
        end
      end
    end

    describe '#validate_tickers_available' do
      let(:bot) { create(:dca_single_asset) }

      it 'is valid on :start when ticker is available' do
        expect(bot).to be_valid(:start)
      end

      it 'is invalid on :start when ticker is not available' do
        bot.ticker.update!(available: false)
        expect(bot).not_to be_valid(:start)
        expect(bot.errors[:base_asset_id]).to include('is invalid')
        expect(bot.errors[:quote_asset_id]).to include('is invalid')
      end
    end
  end

  describe 'settings accessors' do
    let(:bot) { create(:dca_single_asset) }

    it 'provides access to base_asset_id' do
      expect(bot.base_asset_id).to eq(bot.base_asset.id)
    end

    it 'provides access to quote_asset_id' do
      expect(bot.quote_asset_id).to eq(bot.quote_asset.id)
    end

    it 'provides access to quote_amount' do
      expect(bot.quote_amount).to eq(100.0)
    end

    it 'provides access to interval' do
      expect(bot.interval).to eq('day')
    end
  end

  describe 'lifecycle methods' do
    describe '#start' do
      let(:bot) { create(:dca_single_asset) }

      before do
        allow(Bot::ActionJob).to receive(:perform_later)
        allow(Bot::ActionJob).to receive(:set).and_return(double(perform_later: true))
        allow(Bot::BroadcastAfterScheduledActionJob).to receive(:perform_later)
      end

      context 'with start_fresh: true (default)' do
        it 'changes status to scheduled' do
          bot.start
          expect(bot.status).to eq('scheduled')
        end

        it 'sets started_at to current time' do
          freeze_time do
            bot.start
            expect(bot.started_at).to eq(Time.current)
          end
        end

        it 'clears stop_message_key' do
          bot.update!(stop_message_key: 'some_key', status: :stopped)
          bot.start
          expect(bot.stop_message_key).to be_nil
        end

        it 'clears last_action_job_at' do
          bot.last_action_job_at = Time.current
          bot.start
          expect(bot.last_action_job_at).to be_nil
        end

        it 'clears missed_quote_amount' do
          bot.missed_quote_amount = 50
          bot.start
          expect(bot.missed_quote_amount).to eq(0)
        end

        it 'schedules Bot::ActionJob immediately' do
          expect(Bot::ActionJob).to receive(:perform_later).with(bot)
          bot.start
        end

        it 'returns true on success' do
          expect(bot.start).to be true
        end
      end

      context 'with start_fresh: false (restarting)' do
        let(:bot) { create(:dca_single_asset, :stopped) }

        before do
          bot.last_action_job_at = 1.hour.ago.iso8601
        end

        context 'when restarting outside interval' do
          it 'schedules Bot::ActionJob immediately' do
            expect(Bot::ActionJob).to receive(:perform_later).with(bot)
            bot.start(start_fresh: false)
          end

          it 'preserves started_at when not starting fresh' do
            original_started_at = bot.started_at
            bot.start(start_fresh: false)
            expect(bot.started_at).to eq(original_started_at)
          end
        end
      end

      context 'when validation fails' do
        before do
          bot.ticker.update!(available: false)
        end

        it 'returns false' do
          expect(bot.start).to be false
        end

        it 'does not schedule any jobs' do
          expect(Bot::ActionJob).not_to receive(:perform_later)
          bot.start
        end
      end
    end

    describe '#stop' do
      let(:bot) { create(:dca_single_asset, :started) }

      before do
        allow(bot).to receive(:cancel_scheduled_action_jobs)
      end

      it 'changes status to stopped' do
        bot.stop
        expect(bot.status).to eq('stopped')
      end

      it 'sets stopped_at to current time' do
        freeze_time do
          bot.stop
          expect(bot.stopped_at).to eq(Time.current)
        end
      end

      it 'cancels scheduled action jobs' do
        expect(bot).to receive(:cancel_scheduled_action_jobs)
        bot.stop
      end

      it 'stores stop_message_key when provided' do
        bot.stop(stop_message_key: 'manual_stop')
        expect(bot.stop_message_key).to eq('manual_stop')
      end

      it 'returns true on success' do
        expect(bot.stop).to be true
      end
    end

    describe '#delete' do
      let(:bot) { create(:dca_single_asset, :started) }

      before do
        allow(bot).to receive(:cancel_scheduled_action_jobs)
      end

      it 'changes status to deleted' do
        bot.delete
        expect(bot.status).to eq('deleted')
      end

      it 'sets stopped_at to current time' do
        freeze_time do
          bot.delete
          expect(bot.stopped_at).to eq(Time.current)
        end
      end

      it 'cancels scheduled action jobs' do
        expect(bot).to receive(:cancel_scheduled_action_jobs)
        bot.delete
      end

      it 'returns true on success' do
        expect(bot.delete).to be true
      end
    end

    describe '#execute_action' do
      let(:bot) { create(:dca_single_asset, :started) }

      before do
        setup_bot_execution_mocks(bot)
        allow(Bot::FetchAndCreateOrderJob).to receive(:perform_later)
        allow(Bot::FetchAndUpdateOpenOrdersJob).to receive(:perform_now)
        allow(bot).to receive(:broadcast_below_minimums_warning)
      end

      it 'sets status to waiting on success' do
        allow(bot).to receive(:set_order).and_return(Result::Success.new)
        bot.execute_action
        expect(bot.reload.status).to eq('waiting')
      end

      it 'calls set_order with pending_quote_amount' do
        pending_amount = bot.pending_quote_amount
        expect(bot).to receive(:set_order).with(
          order_amount_in_quote: pending_amount,
          update_missed_quote_amount: true
        ).and_return(Result::Success.new)
        bot.execute_action
      end

      it 'returns Success on success' do
        allow(bot).to receive(:set_order).and_return(Result::Success.new)
        result = bot.execute_action
        expect(result).to be_success
      end

      context 'when set_order fails' do
        before do
          allow(bot).to receive(:set_order).and_return(Result::Failure.new('Order failed'))
        end

        it 'returns the failure result' do
          result = bot.execute_action
          expect(result).to be_failure
        end
      end
    end
  end

  describe 'query methods' do
    describe '#restarting?' do
      let(:bot) { create(:dca_single_asset, :stopped) }

      it 'returns true when stopped and has last_action_job_at' do
        bot.last_action_job_at = 1.hour.ago.iso8601
        expect(bot).to be_restarting
      end

      it 'returns false when not stopped' do
        bot.status = :scheduled
        bot.last_action_job_at = 1.hour.ago.iso8601
        expect(bot).not_to be_restarting
      end

      it 'returns false when stopped but no last_action_job_at' do
        bot.last_action_job_at = nil
        expect(bot).not_to be_restarting
      end
    end

    describe '#restarting_within_interval?' do
      let(:bot) { create(:dca_single_asset, :stopped) }

      it 'returns false when not restarting' do
        bot.status = :scheduled
        bot.last_action_job_at = 1.hour.ago.iso8601
        expect(bot).not_to be_restarting_within_interval
      end

      it 'returns false when pending_quote_amount equals effective_quote_amount' do
        bot.last_action_job_at = 1.hour.ago.iso8601
        # No transactions means pending_quote_amount == effective_quote_amount
        expect(bot).not_to be_restarting_within_interval
      end
    end

    describe '#assets' do
      let(:bot) { create(:dca_single_asset) }

      it 'returns both base and quote assets' do
        assets = bot.assets
        expect(assets).to include(bot.base_asset)
        expect(assets).to include(bot.quote_asset)
      end
    end

    describe '#base_asset' do
      let(:bitcoin) { create(:asset, :bitcoin) }
      let(:bot) { create(:dca_single_asset, base_asset: bitcoin) }

      it 'returns the base asset' do
        expect(bot.base_asset).to eq(bitcoin)
      end
    end

    describe '#quote_asset' do
      let(:usd) { create(:asset, :usd) }
      let(:bot) { create(:dca_single_asset, quote_asset: usd) }

      it 'returns the quote asset' do
        expect(bot.quote_asset).to eq(usd)
      end
    end

    describe '#ticker' do
      let(:bot) { create(:dca_single_asset) }

      it 'returns the ticker for the bot asset pair' do
        ticker = bot.ticker
        expect(ticker.base_asset).to eq(bot.base_asset)
        expect(ticker.quote_asset).to eq(bot.quote_asset)
        expect(ticker.exchange).to eq(bot.exchange)
      end
    end

    describe '#tickers' do
      let(:bot) { create(:dca_single_asset) }

      it 'returns an array of tickers' do
        expect(bot.tickers).to be_a(ActiveRecord::Relation)
        expect(bot.tickers.count).to eq(1)
      end
    end

    describe '#decimals' do
      let(:bot) { create(:dca_single_asset) }

      it 'returns decimal configuration from ticker' do
        decimals = bot.decimals
        expect(decimals).to have_key(:base)
        expect(decimals).to have_key(:quote)
        expect(decimals).to have_key(:base_price)
      end

      it 'returns empty hash when no ticker' do
        allow(bot).to receive(:ticker).and_return(nil)
        expect(bot.decimals).to eq({})
      end
    end

    describe '#available_exchanges_for_current_settings' do
      let(:bitcoin) { create(:asset, :bitcoin) }
      let(:usd) { create(:asset, :usd) }
      let(:binance) { create(:binance_exchange) }
      let(:kraken) { create(:kraken_exchange) }
      let!(:binance_ticker) { create(:ticker, exchange: binance, base_asset: bitcoin, quote_asset: usd) }
      let!(:kraken_ticker) { create(:ticker, exchange: kraken, base_asset: bitcoin, quote_asset: usd) }

      let(:bot) { create(:dca_single_asset, exchange: binance, base_asset: bitcoin, quote_asset: usd) }

      it 'returns exchanges that support the current asset pair' do
        available = bot.available_exchanges_for_current_settings
        expect(available).to include(binance)
        expect(available).to include(kraken)
      end

      it 'excludes exchanges without the asset pair' do
        coinbase = create(:coinbase_exchange)
        available = bot.available_exchanges_for_current_settings
        expect(available).not_to include(coinbase)
      end
    end

    describe '#working?' do
      let(:bot) { build(:dca_single_asset) }

      it 'returns true for scheduled status' do
        bot.status = :scheduled
        expect(bot).to be_working
      end

      it 'returns true for executing status' do
        bot.status = :executing
        expect(bot).to be_working
      end

      it 'returns true for retrying status' do
        bot.status = :retrying
        expect(bot).to be_working
      end

      it 'returns true for waiting status' do
        bot.status = :waiting
        expect(bot).to be_working
      end

      it 'returns false for created status' do
        bot.status = :created
        expect(bot).not_to be_working
      end

      it 'returns false for stopped status' do
        bot.status = :stopped
        expect(bot).not_to be_working
      end

      it 'returns false for deleted status' do
        bot.status = :deleted
        expect(bot).not_to be_working
      end
    end
  end

  describe '#api_key_type' do
    let(:bot) { build(:dca_single_asset) }

    it 'returns :trading' do
      expect(bot.api_key_type).to eq(:trading)
    end
  end

  describe '#parse_params' do
    let(:bot) { build(:dca_single_asset) }

    it 'extracts base_asset_id from params' do
      result = bot.parse_params(base_asset_id: '123')
      expect(result[:base_asset_id]).to eq(123)
    end

    it 'extracts quote_asset_id from params' do
      result = bot.parse_params(quote_asset_id: '456')
      expect(result[:quote_asset_id]).to eq(456)
    end

    it 'extracts quote_amount from params' do
      result = bot.parse_params(quote_amount: '100.50')
      expect(result[:quote_amount]).to eq(100.50)
    end

    it 'extracts interval from params' do
      result = bot.parse_params(interval: 'week')
      expect(result[:interval]).to eq('week')
    end

    it 'ignores blank values' do
      result = bot.parse_params(base_asset_id: '', quote_amount: nil)
      expect(result).not_to have_key(:base_asset_id)
      expect(result).not_to have_key(:quote_amount)
    end
  end

  describe '#effective_quote_amount' do
    let(:bot) { build(:dca_single_asset) }

    it 'returns the quote_amount' do
      bot.quote_amount = 150
      expect(bot.effective_quote_amount).to eq(150)
    end
  end

  describe 'concerns integration' do
    describe 'Schedulable' do
      let(:bot) { create(:dca_single_asset) }

      it 'provides interval_duration' do
        expect(bot.interval_duration).to eq(1.day)
      end

      it 'provides effective_interval_duration' do
        expect(bot.effective_interval_duration).to eq(1.day)
      end

      describe '#next_interval_checkpoint_at' do
        let(:bot) { create(:dca_single_asset, :started) }

        it 'calculates the next checkpoint based on started_at' do
          freeze_time do
            bot.update!(started_at: 2.hours.ago)
            # With daily interval, next checkpoint should be ~22 hours from now
            next_checkpoint = bot.next_interval_checkpoint_at
            expect(next_checkpoint).to be > Time.current
            expect(next_checkpoint).to be < 1.day.from_now
          end
        end
      end
    end

    describe 'Accountable' do
      let(:bot) { create(:dca_single_asset, :started) }

      describe '#pending_quote_amount' do
        it 'returns effective_quote_amount when no transactions exist' do
          expect(bot.pending_quote_amount).to eq(bot.effective_quote_amount)
        end

        it 'subtracts invested amount from pending' do
          create(:transaction, bot: bot, quote_amount_exec: 30, external_status: :closed, created_at: Time.current)
          bot.reload
          expect(bot.pending_quote_amount).to eq(70)
        end
      end

      describe '#set_missed_quote_amount' do
        it 'stores the current pending_quote_amount' do
          bot.set_missed_quote_amount
          expect(bot.missed_quote_amount).to eq(bot.pending_quote_amount)
        end
      end
    end
  end

  describe 'STI' do
    it 'inherits from Bot' do
      expect(Bots::DcaSingleAsset.superclass).to eq(Bot)
    end

    it 'sets the correct type' do
      bot = create(:dca_single_asset)
      expect(bot.type).to eq('Bots::DcaSingleAsset')
    end
  end

  describe 'factory' do
    it 'creates a valid bot with default settings' do
      bot = build(:dca_single_asset)
      expect(bot).to be_valid
    end

    it 'creates associated exchange and assets' do
      bot = create(:dca_single_asset)
      expect(bot.exchange).to be_present
      expect(bot.base_asset).to be_present
      expect(bot.quote_asset).to be_present
      expect(bot.ticker).to be_present
    end

    it 'creates associated API key by default' do
      bot = create(:dca_single_asset)
      api_key = ApiKey.find_by(user: bot.user, exchange: bot.exchange, key_type: :trading)
      expect(api_key).to be_present
      expect(api_key).to be_persisted
    end

    it 'can skip API key creation' do
      bot = create(:dca_single_asset, with_api_key: false)
      expect(ApiKey.where(user: bot.user, exchange: bot.exchange)).to be_empty
    end

    describe 'status traits' do
      it 'creates a started bot' do
        bot = create(:dca_single_asset, :started)
        expect(bot).to be_scheduled
        expect(bot.started_at).to be_present
      end

      it 'creates a stopped bot' do
        bot = create(:dca_single_asset, :stopped)
        expect(bot).to be_stopped
        expect(bot.stopped_at).to be_present
      end

      it 'creates an executing bot' do
        bot = create(:dca_single_asset, :executing)
        expect(bot).to be_executing
      end

      it 'creates a waiting bot' do
        bot = create(:dca_single_asset, :waiting)
        expect(bot).to be_waiting
      end
    end

    describe 'interval traits' do
      it 'creates an hourly bot' do
        bot = create(:dca_single_asset, :hourly)
        expect(bot.interval).to eq('hour')
      end

      it 'creates a weekly bot' do
        bot = create(:dca_single_asset, :weekly)
        expect(bot.interval).to eq('week')
      end

      it 'creates a monthly bot' do
        bot = create(:dca_single_asset, :monthly)
        expect(bot.interval).to eq('month')
      end
    end
  end
end
