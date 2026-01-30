require 'rails_helper'

RSpec.describe 'Bot::OrderSetter' do
  include ActiveSupport::Testing::TimeHelpers

  # Tests for order creation when amount is below minimum exchange requirements.
  # When the order amount is below the exchange's minimum, the bot should:
  # 1. Create a skipped transaction record
  # 2. NOT submit an order to the exchange
  # 3. Buffer the amount for the next interval

  describe 'below minimum amount handling' do
    describe 'with DcaSingleAsset bot' do
      let(:bot) { create(:dca_single_asset, :started) }
      let(:ticker) { bot.ticker }
      let(:price) { 50000.0 }

      before do
        setup_bot_execution_mocks(bot, price: price)
        allow(bot).to receive(:broadcast_below_minimums_warning)
      end

      context 'when order amount is below minimum' do
        let(:small_amount) { 1.0 } # $1 is below minimum_quote_size of $10

        it 'creates a skipped transaction' do
          expect {
            bot.set_order(order_amount_in_quote: small_amount)
          }.to change { bot.transactions.skipped.count }.by(1)
        end

        it 'does not call exchange API' do
          expect(bot.exchange).not_to receive(:market_buy)
          expect(bot.exchange).not_to receive(:limit_buy)

          bot.set_order(order_amount_in_quote: small_amount)
        end

        it 'returns success' do
          result = bot.set_order(order_amount_in_quote: small_amount)
          expect(result).to be_a(Result::Success)
        end

        it 'records the skipped order with correct data' do
          bot.set_order(order_amount_in_quote: small_amount)

          skipped_txn = bot.transactions.skipped.last
          expect(skipped_txn.status).to eq('skipped')
          expect(skipped_txn.quote_amount).to be_present
          expect(skipped_txn.amount_exec).to eq(0)
          expect(skipped_txn.quote_amount_exec).to eq(0)
        end
      end

      context 'when order amount meets minimum' do
        let(:sufficient_amount) { 100.0 } # $100 is above minimum

        it 'does not create a skipped transaction' do
          expect {
            bot.set_order(order_amount_in_quote: sufficient_amount)
          }.not_to change { bot.transactions.skipped.count }
        end

        it 'calls exchange API' do
          expect(bot.exchange).to receive(:market_buy).and_call_original

          bot.set_order(order_amount_in_quote: sufficient_amount)
        end
      end

      context 'when order amount is zero' do
        it 'returns success without creating any transaction' do
          expect {
            result = bot.set_order(order_amount_in_quote: 0)
            expect(result).to be_a(Result::Success)
          }.not_to change { bot.transactions.count }
        end
      end

      context 'when order amount is exactly at minimum' do
        let(:exact_minimum) { ticker.minimum_quote_size }

        it 'submits the order (not skipped)' do
          expect(bot.exchange).to receive(:market_buy).and_call_original

          bot.set_order(order_amount_in_quote: exact_minimum)
        end
      end
    end

    describe 'with DcaDualAsset bot' do
      let(:bot) { create(:dca_dual_asset, :started) }
      let(:ticker0) { bot.ticker0 }
      let(:ticker1) { bot.ticker1 }
      let(:price) { 50000.0 }

      before do
        setup_bot_execution_mocks(bot, price: price)
        allow(bot).to receive(:broadcast_below_minimums_warning)
        # Mock metrics to return zero balances (forces full allocation to orders)
        allow(bot).to receive(:metrics).and_return({
          total_base0_amount: 0,
          total_base1_amount: 0
        })
      end

      context 'when order amount is below minimum' do
        let(:small_amount) { 1.0 } # $1 is below minimum_quote_size of $10

        it 'creates skipped transactions' do
          expect {
            bot.set_orders(total_orders_amount_in_quote: small_amount)
          }.to change { bot.transactions.skipped.count }.by_at_least(1)
        end

        it 'does not call exchange API' do
          expect(bot.exchange).not_to receive(:market_buy)
          expect(bot.exchange).not_to receive(:limit_buy)

          bot.set_orders(total_orders_amount_in_quote: small_amount)
        end

        it 'returns success' do
          result = bot.set_orders(total_orders_amount_in_quote: small_amount)
          expect(result).to be_a(Result::Success)
        end
      end

      context 'when order amount meets minimum' do
        let(:sufficient_amount) { 100.0 } # $100 is above minimum

        it 'calls exchange API' do
          expect(bot.exchange).to receive(:market_buy).at_least(:once).and_call_original

          bot.set_orders(total_orders_amount_in_quote: sufficient_amount)
        end
      end

      context 'when order amount is zero' do
        it 'returns success without creating any transaction' do
          expect {
            result = bot.set_orders(total_orders_amount_in_quote: 0)
            expect(result).to be_a(Result::Success)
          }.not_to change { bot.transactions.count }
        end
      end
    end
  end

  describe 'buffer accumulation across intervals' do
    # Tests that amounts below minimum accumulate via pending_quote_amount
    # until they reach the minimum threshold

    let(:bot) do
      # Create bot with small quote_amount (below minimum of 10)
      bot = build(:dca_single_asset, :started)
      bot.settings = bot.settings.merge('quote_amount' => 5.0)
      bot.set_missed_quote_amount
      bot.save!
      bot
    end
    let(:ticker) { bot.ticker }

    before do
      setup_bot_execution_mocks(bot, price: 50000)
      allow(bot).to receive(:broadcast_below_minimums_warning)
    end

    it 'accumulates pending amount when orders are skipped' do
      # First interval - amount too small, gets skipped
      initial_pending = bot.pending_quote_amount
      expect(initial_pending).to eq(5.0)

      bot.set_order(order_amount_in_quote: initial_pending)

      # Transaction was skipped because 5.0 < minimum_quote_size (10)
      expect(bot.transactions.skipped.count).to eq(1)
    end

    it 'pending_quote_amount increases over multiple intervals' do
      # Start with one interval's worth
      initial_pending = bot.pending_quote_amount
      expect(initial_pending).to eq(5.0) # quote_amount for first interval

      # Advance time past one full interval (into second interval)
      # Use 25 hours to be safely into the next day's interval
      travel 25.hours do
        # Now we should have 2 intervals worth of pending amount
        expect(bot.pending_quote_amount).to eq(10.0)
      end
    end

    it 'executes order when accumulated amount reaches minimum' do
      # Advance time to accumulate enough for minimum (2 intervals)
      travel 25.hours do
        pending = bot.pending_quote_amount
        expect(pending).to eq(10.0)

        # This should now execute (not skip) - meets minimum
        expect(bot.exchange).to receive(:market_buy).and_call_original

        bot.set_order(order_amount_in_quote: pending)

        # No skipped transactions - order was executed
        # (actual transaction record is created async by FetchAndCreateOrderJob)
        expect(bot.transactions.skipped.count).to eq(0)
      end
    end
  end

  describe 'missed_quote_amount buffer' do
    let(:bot) { create(:dca_single_asset, :started) }

    it 'preserves pending amount when settings change' do
      # Create a transaction that partially fills the interval
      create(:transaction, bot: bot, quote_amount_exec: 30, external_status: :closed, created_at: Time.current)
      bot.reload

      # pending_quote_amount should reflect unfilled portion
      expect(bot.pending_quote_amount).to eq(70) # 100 - 30

      # When settings change, we need to preserve the pending amount
      bot.set_missed_quote_amount
      expect(bot.missed_quote_amount).to eq(70)

      # Now change quote_amount
      bot.update!(settings: bot.settings.merge('quote_amount' => 200.0))

      # The missed_quote_amount carries forward
      expect(bot.missed_quote_amount).to eq(70)
    end

    it 'clears missed_quote_amount on bot start' do
      bot.update!(missed_quote_amount: 50.0, status: :stopped)

      bot.start

      expect(bot.missed_quote_amount).to eq(0)
    end
  end
end
