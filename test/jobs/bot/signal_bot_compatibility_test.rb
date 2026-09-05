require 'test_helper'

# Bots::Signal is passive: no Automation::Schedulable, no Bot::Accountable. Every job shared across
# bot types that reaches for a scheduling or carry method has to tolerate that, because a started
# signal bot sits in :scheduled and its market orders sit in :unknown like any other bot's — and
# one NoMethodError in a fleet-wide sweep switches the sweep off for every DCA bot too.
class Bot::SignalBotCompatibilityTest < ActiveSupport::TestCase
  setup do
    SolidQueue::Job.destroy_all
    SolidQueue::ScheduledExecution.destroy_all
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
  end

  test 'orphan repair skips a started signal bot and still repairs the DCA bot beside it' do
    signal_bot = create(:signal_bot, :started)
    dca_bot = create(:dca_single_asset, :started, status: :scheduled, user: signal_bot.user,
                                                  exchange: signal_bot.exchange, base_asset: signal_bot.base_asset,
                                                  quote_asset: signal_bot.quote_asset)
    assert_nil dca_bot.next_action_job_at

    assert_nothing_raised { Bot::RepairOrphanedBotsJob.perform_now }

    assert dca_bot.reload.next_action_job_at.present?, 'the DCA bot must still be re-armed'
    assert_equal 1, SolidQueue::Job.where(class_name: 'Bot::ActionJob').count, 'no ActionJob for the signal bot'
  end

  # The cancel button, the CSV export and the on-connect open-orders sweep all pass
  # update_missed_quote_amount: true; the buy-side carry they adjust does not exist on a signal bot.
  test 'confirming a signal buy with the carry flag set does not raise' do
    bot = create(:signal_bot, :started)
    txn = create(:transaction, bot: bot, side: :buy, status: :submitted, external_status: :unknown,
                               external_id: 'sig-u1', amount_exec: nil, quote_amount_exec: nil)
    closed = { status: :closed, price: 50_000, amount: 0.002, quote_amount: 100, amount_exec: 0.002,
               quote_amount_exec: 100, ticker: bot.ticker, side: :buy, order_type: :market_order }
    bot.stubs(:get_order).returns(Result::Success.new(closed))
    txn.stubs(:bot).returns(bot)

    assert_nothing_raised { Bot::FetchAndUpdateOrderJob.new.perform(txn, update_missed_quote_amount: true) }
    assert_equal 'closed', txn.reload.external_status
  end

  test 'the open-orders sweep with the carry flag set confirms every waiting signal buy' do
    bot = create(:signal_bot, :started)
    first = create(:transaction, bot: bot, side: :buy, status: :submitted, external_status: :unknown,
                                 external_id: 'sig-s1', amount_exec: nil, quote_amount_exec: nil)
    second = create(:transaction, bot: bot, side: :buy, status: :submitted, external_status: :unknown,
                                  external_id: 'sig-s2', amount_exec: nil, quote_amount_exec: nil)
    closed = { status: :closed, price: 50_000, amount: 0.002, quote_amount: 100, amount_exec: 0.002,
               quote_amount_exec: 100, ticker: bot.ticker, side: :buy, order_type: :market_order }
    bot.stubs(:get_orders).returns(Result::Success.new(orders: { 'sig-s1' => closed, 'sig-s2' => closed }, missing: []))

    assert_nothing_raised { Bot::FetchAndUpdateOpenOrdersJob.new.perform(bot, update_missed_quote_amount: true) }

    assert_equal 'closed', first.reload.external_status
    assert_equal 'closed', second.reload.external_status
  end

  # The app-wake broadcast waits up to five seconds for a next tick that a signal bot never has.
  test 'the after-scheduled broadcast does not wait on a signal bot' do
    bot = create(:signal_bot, :started)
    job = Bot::BroadcastAfterScheduledActionJob.new
    job.expects(:sleep).never
    bot.expects(:broadcast_status_bar_update)

    job.perform(bot)
  end
end
