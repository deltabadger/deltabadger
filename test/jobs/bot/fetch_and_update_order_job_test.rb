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
end
