require 'test_helper'

# Taking the offer off the table. The job exists so this runs under the exchange semaphore — see
# Bot::DeclineRedeployJob for why guarding from the request was not enough.
class Bot::DeclineRedeployJobTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_index, user: create(:user), with_api_key: true)
    asset = create(:asset, symbol: 'AAA', name: 'Coin AAA', external_id: 'coin-aaa')
    create(:ticker, exchange: @bot.exchange, base_asset: asset, quote_asset: @bot.quote_asset)
  end

  test 'it writes the offset that takes the current proceeds off the table' do
    liquidated(150)
    # The repaint reads live prices; this test is about the offset, not the broadcast.
    @bot.stubs(:broadcast_redeploy_state)

    Bot::DeclineRedeployJob.new.perform(@bot)

    assert_in_delta 150, @bot.reload.redeploy_declined_offset.to_d.to_f, 0.0001
    assert @bot.bot_activity_logs.exists?(event: 'redeploy_declined')
  end

  # The window the semaphore closes: an offset snapshotted while `spent` is still growing strands
  # above the banked total and silently eats every later sale.
  test 'it refuses while a batch is still working, and says so' do
    liquidated(150)
    waiting_redeploy

    Bot::DeclineRedeployJob.new.perform(@bot)

    assert_in_delta 0, @bot.reload.redeploy_declined_offset.to_d.to_f, 0.0001
    assert @bot.bot_activity_logs.exists?(event: 'redeploy_decline_refused')
  end

  test 'it shares the exchange semaphore with the placement leg' do
    assert_equal 'Bot::ActionJob', Bot::DeclineRedeployJob.concurrency_group
    assert_equal Bot::RedeployJob.concurrency_key.call(@bot),
                 Bot::DeclineRedeployJob.concurrency_key.call(@bot)
  end

  test 'a bot type without the leg is left alone' do
    other = create(:dca_single_asset, user: @bot.user)

    Bot::DeclineRedeployJob.new.perform(other)

    assert_empty other.bot_activity_logs
  end

  private

  def liquidated(quote)
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                         external_status: :closed, external_id: "l-#{SecureRandom.hex(4)}",
                         side: :sell, transaction_type: 'LIQUIDATION', base: 'AAA',
                         quote: @bot.quote_asset.symbol, price: 100, amount: quote.to_d / 100,
                         amount_exec: quote.to_d / 100, quote_amount: quote, quote_amount_exec: quote)
  end

  def waiting_redeploy
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                         external_status: :open, external_id: "w-#{SecureRandom.hex(4)}",
                         side: :buy, transaction_type: 'REDEPLOY', base: 'AAA',
                         quote: @bot.quote_asset.symbol, price: 100, amount: 1,
                         amount_exec: 0, quote_amount: 100, quote_amount_exec: 0)
  end
end
