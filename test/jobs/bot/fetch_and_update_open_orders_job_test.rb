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
    bot.stubs(:get_orders).returns(Result::Success.new(orders: { 'u1' => order_data }, missing: []))

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

    error = assert_raises(RuntimeError) do
      Bot::FetchAndUpdateOpenOrdersJob.new.perform(bot)
    end

    assert_match(/exchange down/, error.message)
  end

  # == stale-order handling on the bulk path ==
  # When Kraken silently drops an aged-out order ID, it now arrives in
  # result.data[:missing]. The job hands each one to Bot::StaleOrderResolver:
  # old → :abandoned + activity log; young → :too_young (no mutation).

  test 'marks old missing orders :abandoned and logs order_abandoned in one bulk invocation' do
    bot = create(:dca_single_asset, :started)
    fresh = create(:transaction, bot: bot, status: :submitted, external_status: :unknown,
                                 external_id: 'fresh', amount_exec: nil, quote_amount_exec: nil)
    stale = create(:transaction, bot: bot, status: :submitted, external_status: :unknown,
                                 external_id: 'stale',
                                 created_at: (Bot::StaleOrderResolver::STALE_ORDER_THRESHOLD + 1.day).ago)

    fresh_order_data = {
      status: :closed, price: 50_000, amount: 0.002, quote_amount: 100,
      amount_exec: 0.002, quote_amount_exec: 100,
      ticker: bot.ticker, side: :buy, order_type: :market_order
    }
    bot.stubs(:get_orders).returns(
      Result::Success.new(orders: { 'fresh' => fresh_order_data }, missing: %w[stale])
    )

    assert_difference -> { bot.bot_activity_logs.where(event: 'order_abandoned').count }, 1 do
      Bot::FetchAndUpdateOpenOrdersJob.new.perform(bot)
    end

    assert_equal 'closed', fresh.reload.external_status
    assert_equal 'abandoned', stale.reload.external_status
    log = bot.bot_activity_logs.where(event: 'order_abandoned').last
    assert_equal 'stale', log.details['order_id']
  end

  test 'still raises loudly when a young missing ID appears in the bulk batch' do
    # Symmetry with FetchAndUpdateOrderJob: a fresh order that the exchange
    # silently drops is almost certainly a real bug (wrong key, subaccount
    # mismatch), not retention drift. Surface it instead of no-opping.
    bot = create(:dca_single_asset, :started)
    young_missing = create(:transaction, bot: bot, status: :submitted, external_status: :unknown,
                                         external_id: 'young_missing', created_at: 1.day.ago)
    bot.stubs(:get_orders).returns(Result::Success.new(orders: {}, missing: %w[young_missing]))

    error = assert_raises(RuntimeError) { Bot::FetchAndUpdateOpenOrdersJob.new.perform(bot) }

    assert_match(/omitted recent order/i, error.message)
    assert_match(/young_missing/, error.message)
    assert_equal 'unknown', young_missing.reload.external_status
  end

  test 'processes old missing orders and found orders before raising on young missing IDs in the same batch' do
    # Partial-progress semantics: don't lose a fresh confirmation or block a
    # legitimate abandonment because a different ID in the same batch is suspect.
    bot = create(:dca_single_asset, :started)
    fresh = create(:transaction, bot: bot, status: :submitted, external_status: :unknown,
                                 external_id: 'fresh', amount_exec: nil, quote_amount_exec: nil)
    stale = create(:transaction, bot: bot, status: :submitted, external_status: :unknown,
                                 external_id: 'stale',
                                 created_at: (Bot::StaleOrderResolver::STALE_ORDER_THRESHOLD + 1.day).ago)
    young_missing = create(:transaction, bot: bot, status: :submitted, external_status: :unknown,
                                         external_id: 'young_missing', created_at: 1.day.ago)

    fresh_order_data = {
      status: :closed, price: 50_000, amount: 0.002, quote_amount: 100,
      amount_exec: 0.002, quote_amount_exec: 100,
      ticker: bot.ticker, side: :buy, order_type: :market_order
    }
    bot.stubs(:get_orders).returns(
      Result::Success.new(orders: { 'fresh' => fresh_order_data },
                          missing: %w[stale young_missing])
    )

    error = assert_raises(RuntimeError) { Bot::FetchAndUpdateOpenOrdersJob.new.perform(bot) }

    assert_match(/omitted recent order/i, error.message)
    assert_match(/young_missing/, error.message)
    assert_equal 'closed', fresh.reload.external_status, 'fresh confirmation must land before the raise'
    assert_equal 'abandoned', stale.reload.external_status, 'old missing must be abandoned before the raise'
    assert_equal 'unknown', young_missing.reload.external_status
  end

  # == transient Kraken errors (HTTP 200 + error array) ==
  # On the bulk path, a Kraken transient failure must raise Client::TransientNetworkError
  # so it funnels into ActionJob's retry_on (this job runs perform_now, inline in
  # execute_action). NOTE: bot MUST be on Kraken (default factory exchange has no :transient).

  test 'raises Client::TransientNetworkError for a Kraken transient bulk failure' do
    bot = create(:dca_single_asset, :started, exchange: create(:kraken_exchange))
    create(:transaction, bot: bot, status: :submitted, external_status: :open, external_id: 'o1')
    bot.stubs(:get_orders).returns(Result::Failure.new('EGeneral:Internal error'))

    error = assert_raises(Client::TransientNetworkError) { Bot::FetchAndUpdateOpenOrdersJob.new.perform(bot) }
    assert_match(/EGeneral:Internal error/, error.message)
  end

  test 'still raises a plain RuntimeError (not TransientNetworkError) for a non-transient failure' do
    bot = create(:dca_single_asset, :started, exchange: create(:kraken_exchange))
    create(:transaction, bot: bot, status: :submitted, external_status: :open, external_id: 'o1')
    bot.stubs(:get_orders).returns(Result::Failure.new('exchange down'))

    error = assert_raises(RuntimeError) { Bot::FetchAndUpdateOpenOrdersJob.new.perform(bot) }
    refute_kind_of Client::TransientNetworkError, error
  end

  test 'suppresses a transient failure under success_or_kill (controller path)' do
    bot = create(:dca_single_asset, :started, exchange: create(:kraken_exchange))
    create(:transaction, bot: bot, status: :submitted, external_status: :open, external_id: 'o1')
    bot.stubs(:get_orders).returns(Result::Failure.new('EAPI:Invalid nonce'))

    assert_nothing_raised do
      Bot::FetchAndUpdateOpenOrdersJob.new.perform(bot, success_or_kill: true)
    end
  end

  test 'does not declare its own retry_on (relies on ActionJob via inline perform_now)' do
    handler_classes = Bot::FetchAndUpdateOpenOrdersJob.rescue_handlers.map(&:first)
    refute_includes handler_classes, 'Client::TransientNetworkError'
  end

  # End-to-end funnel: the inline sweep (limit_orderable decorator) raises the
  # transient error OUT of execute_action, where ActionJob's rescue Client::TransientNetworkError
  # picks it up (proven separately in action_job_test). Driving execute_action directly
  # avoids mocking auth/market/scheduling.
  test 'inline sweep raises a Kraken transient error out of execute_action' do
    bot = create(:dca_single_asset, :started, exchange: create(:kraken_exchange))
    create(:transaction, bot: bot, status: :submitted, external_status: :open, external_id: 'o1')
    bot.stubs(:get_orders).returns(Result::Failure.new('EAPI:Invalid nonce'))

    assert_raises(Client::TransientNetworkError) { bot.execute_action }
  end
end
