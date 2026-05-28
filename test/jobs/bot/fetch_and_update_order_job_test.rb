require 'test_helper'

class Bot::FetchAndUpdateOrderJobTest < ActiveSupport::TestCase
  # This is the job Finding 1 hands off to after persisting a submitted/unknown row.
  # It must fill in execution amounts on confirmation, and — critically — never
  # destroy the durable row when confirmation keeps failing.

  test 'updates a submitted/unknown transaction to closed with execution amounts' do
    bot = create(:dca_single_asset, :started)
    txn = create(:transaction, bot: bot, status: :submitted, external_status: :unknown,
                               external_id: 'u1', amount_exec: nil, quote_amount_exec: nil)
    txn.stubs(:bot).returns(bot)

    order_data = {
      status: :closed, price: 50_000, amount: 0.002, quote_amount: 100,
      amount_exec: 0.002, quote_amount_exec: 100,
      ticker: bot.ticker, side: :buy, order_type: :market_order
    }
    bot.stubs(:get_order).returns(Result::Success.new(order_data))

    Bot::FetchAndUpdateOrderJob.new.perform(txn)

    txn.reload
    assert_equal 'closed', txn.external_status
    assert_equal 0.002, txn.amount_exec
    assert_equal 100, txn.quote_amount_exec
  end

  test 'raises on persistent unknown status but leaves the transaction intact' do
    bot = create(:dca_single_asset, :started)
    txn = create(:transaction, bot: bot, status: :submitted, external_status: :unknown, external_id: 'u1')
    txn.stubs(:bot).returns(bot)
    bot.stubs(:get_order).returns(Result::Success.new({ status: :unknown, quote_amount_exec: 0, ticker: bot.ticker }))

    assert_raises(RuntimeError) { Bot::FetchAndUpdateOrderJob.new.perform(txn) }

    assert Transaction.exists?(txn.id)
    assert_equal 'unknown', txn.reload.external_status
  end

  test 'suppresses errors under success_or_kill' do
    bot = create(:dca_single_asset, :started)
    txn = create(:transaction, bot: bot, status: :submitted, external_status: :unknown, external_id: 'u1')
    txn.stubs(:bot).returns(bot)
    bot.stubs(:get_order).returns(Result::Failure.new('exchange down'))

    assert_nothing_raised { Bot::FetchAndUpdateOrderJob.new.perform(txn, success_or_kill: true) }
  end

  # == stale-order handling (not_found signal from Kraken) ==

  test 'marks an old order :abandoned and logs an order_abandoned activity when the exchange reports not_found' do
    bot = create(:dca_single_asset, :started)
    old = (Bot::StaleOrderResolver::STALE_ORDER_THRESHOLD + 1.day).ago
    txn = create(:transaction, bot: bot, status: :submitted, external_status: :unknown,
                               external_id: 'TXID-STALE', created_at: old)
    txn.stubs(:bot).returns(bot)
    bot.stubs(:get_order).returns(Result::Failure.new('Kraken did not return data', data: { not_found: true }))

    assert_difference -> { bot.bot_activity_logs.where(event: 'order_abandoned').count }, 1 do
      assert_nothing_raised { Bot::FetchAndUpdateOrderJob.new.perform(txn) }
    end

    assert_equal 'abandoned', txn.reload.external_status
    log = bot.bot_activity_logs.where(event: 'order_abandoned').last
    assert_equal 'TXID-STALE', log.details['order_id']
  end

  test 'still raises on a young order with not_found so real bugs (wrong key, etc.) remain loud' do
    bot = create(:dca_single_asset, :started)
    txn = create(:transaction, bot: bot, status: :submitted, external_status: :unknown,
                               external_id: 'TXID-YOUNG', created_at: 1.day.ago)
    txn.stubs(:bot).returns(bot)
    bot.stubs(:get_order).returns(Result::Failure.new('Kraken did not return data', data: { not_found: true }))

    assert_raises(RuntimeError) { Bot::FetchAndUpdateOrderJob.new.perform(txn) }
    assert_equal 'unknown', txn.reload.external_status
  end

  test 'still raises on a generic failure without the not_found flag, preserving existing behavior' do
    bot = create(:dca_single_asset, :started)
    txn = create(:transaction, bot: bot, status: :submitted, external_status: :unknown, external_id: 'u1')
    txn.stubs(:bot).returns(bot)
    bot.stubs(:get_order).returns(Result::Failure.new('exchange down'))

    assert_raises(RuntimeError) { Bot::FetchAndUpdateOrderJob.new.perform(txn) }
  end
end
