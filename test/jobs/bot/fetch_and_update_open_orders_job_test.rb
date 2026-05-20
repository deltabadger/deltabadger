require 'test_helper'

class Bot::FetchAndUpdateOpenOrdersJobTest < ActiveSupport::TestCase
  # Orphan regression: a submitted/unknown order (Finding 1) must be picked up by
  # the open-order refresher, not just rows whose external_status is already :open.
  # Otherwise an order whose first confirmation fetch failed sits unconfirmed forever.

  test 'refreshes submitted/unknown orders, not only open ones' do
    bot = create(:dca_single_asset, :started)
    txn = create(:transaction, bot: bot, status: :submitted, external_status: :unknown,
                               external_id: 'u1', amount_exec: nil, quote_amount_exec: nil)

    order_data = {
      status: :closed, price: 50_000, amount: 0.002, quote_amount: 100,
      amount_exec: 0.002, quote_amount_exec: 100,
      ticker: bot.ticker, side: :buy, order_type: :market_order
    }
    bot.stubs(:get_orders).returns(Result::Success.new({ 'u1' => order_data }))

    Bot::FetchAndUpdateOpenOrdersJob.new.perform(bot)

    txn.reload
    assert_equal 'closed', txn.external_status
    assert_equal 0.002, txn.amount_exec
    assert_equal 100, txn.quote_amount_exec
  end

  # Finding 2: when success_or_kill is set, failures must be logged (structured)
  # before being suppressed — not silently swallowed.

  test 'logs a structured warning before suppressing a failure under success_or_kill' do
    bot = create(:dca_single_asset, :started)
    create(:transaction, bot: bot, status: :submitted, external_status: :open, external_id: 'o1')
    bot.stubs(:get_orders).returns(Result::Failure.new('exchange down'))

    logged = nil
    Rails.logger.stubs(:warn).with do |msg|
      logged = msg
      true
    end

    assert_nothing_raised do
      Bot::FetchAndUpdateOpenOrdersJob.new.perform(bot, success_or_kill: true)
    end

    assert_not_nil logged, 'expected a warn line'
    assert_match(/bot_id=#{bot.id}/, logged)
    assert_match(/order_ids=/, logged)
  end

  test 'still raises when success_or_kill is not set' do
    bot = create(:dca_single_asset, :started)
    create(:transaction, bot: bot, status: :submitted, external_status: :open, external_id: 'o1')
    bot.stubs(:get_orders).returns(Result::Failure.new('exchange down'))

    assert_raises(RuntimeError) do
      Bot::FetchAndUpdateOpenOrdersJob.new.perform(bot)
    end
  end
end
