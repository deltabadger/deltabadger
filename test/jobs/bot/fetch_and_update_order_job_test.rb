require 'test_helper'

class Bot::FetchAndUpdateOrderJobTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

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

    error = assert_raises(RuntimeError) { Bot::FetchAndUpdateOrderJob.new.perform(txn) }

    assert_match(/status is unknown/i, error.message)
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

    error = assert_raises(RuntimeError) { Bot::FetchAndUpdateOrderJob.new.perform(txn) }

    assert_match(/Failed to fetch order #{txn.id}/, error.message)
    assert_equal 'unknown', txn.reload.external_status
  end

  test 'still raises on a generic failure without the not_found flag, preserving existing behavior' do
    bot = create(:dca_single_asset, :started)
    txn = create(:transaction, bot: bot, status: :submitted, external_status: :unknown, external_id: 'u1')
    txn.stubs(:bot).returns(bot)
    bot.stubs(:get_order).returns(Result::Failure.new('exchange down'))

    error = assert_raises(RuntimeError) { Bot::FetchAndUpdateOrderJob.new.perform(txn) }

    assert_match(/exchange down/, error.message)
  end

  # == transient Kraken errors (HTTP 200 + error array) ==
  # A Kraken transient failure must raise Client::TransientNetworkError so the
  # job's retry_on backs it off, instead of failing the job with a bare RuntimeError.
  # NOTE: the bot MUST be on Kraken — the default factory exchange (Binance) has no
  # :transient known_errors, so transient_error? would return false for the wrong reason.

  test 'raises Client::TransientNetworkError for a Kraken internal-error failure' do
    bot = create(:dca_single_asset, :started, exchange: create(:kraken_exchange))
    txn = create(:transaction, bot: bot, status: :submitted, external_status: :unknown, external_id: 'u1')
    txn.stubs(:bot).returns(bot)
    bot.stubs(:get_order).returns(Result::Failure.new('EGeneral:Internal error'))

    error = assert_raises(Client::TransientNetworkError) { Bot::FetchAndUpdateOrderJob.new.perform(txn) }
    assert_match(/EGeneral:Internal error/, error.message)
  end

  test 'raises Client::TransientNetworkError for a Kraken invalid-nonce failure' do
    bot = create(:dca_single_asset, :started, exchange: create(:kraken_exchange))
    txn = create(:transaction, bot: bot, status: :submitted, external_status: :unknown, external_id: 'u1')
    txn.stubs(:bot).returns(bot)
    bot.stubs(:get_order).returns(Result::Failure.new('EAPI:Invalid nonce'))

    assert_raises(Client::TransientNetworkError) { Bot::FetchAndUpdateOrderJob.new.perform(txn) }
  end

  # A Kraken rate-limit failure must raise Client::RateLimitedError (its own retry path
  # with a longer, escalating wait) — NOT a bare RuntimeError that fails the job, and
  # NOT a TransientNetworkError (which would back off too fast and re-trip the limit).
  test 'raises Client::RateLimitedError for a Kraken rate-limit failure' do
    bot = create(:dca_single_asset, :started, exchange: create(:kraken_exchange))
    txn = create(:transaction, bot: bot, status: :submitted, external_status: :unknown, external_id: 'u1')
    txn.stubs(:bot).returns(bot)
    bot.stubs(:get_order).returns(Result::Failure.new('EAPI:Rate limit exceeded'))

    error = assert_raises(Client::RateLimitedError) { Bot::FetchAndUpdateOrderJob.new.perform(txn) }
    refute_kind_of Client::TransientNetworkError, error
    assert_match(/EAPI:Rate limit exceeded/, error.message)
  end

  # Proves the throttle check runs BEFORE the transient check: when BOTH predicates
  # would match, rate-limit wins (RateLimitedError, with its longer wait). Without this
  # the ordering is untestable via real codes — no code is both throttle and transient.
  test 'throttle takes precedence over transient when both classifiers match' do
    bot = create(:dca_single_asset, :started, exchange: create(:kraken_exchange))
    txn = create(:transaction, bot: bot, status: :submitted, external_status: :unknown, external_id: 'u1')
    txn.stubs(:bot).returns(bot)
    bot.stubs(:get_order).returns(Result::Failure.new('ambiguous failure'))
    bot.exchange.stubs(:throttled_error?).returns(true)
    bot.exchange.stubs(:transient_error?).returns(true)

    assert_raises(Client::RateLimitedError) { Bot::FetchAndUpdateOrderJob.new.perform(txn) }
  end

  # Proves retry_on is actually WIRED to the escalating BotJob::RATE_LIMIT_WAIT, not just
  # that the lambda exists: the retry is rescheduled at 15s on attempt 1 and 30s on
  # attempt 2. A hardcoded fixed `wait:` would reschedule at 15s both times and fail the
  # 30s assertion. perform_now lets retry_on rescue + re-enqueue; this repo runs the
  # SolidQueue adapter in tests, so inspect the persisted SolidQueue::Job (matching
  # Bot::ActionJobSchedulingIntegrationTest's pattern) rather than the :test adapter.
  test 'retry_on reschedules a rate-limited fetch using the escalating wait' do
    bot = create(:dca_single_asset, :started, exchange: create(:kraken_exchange))
    txn = create(:transaction, bot: bot, status: :submitted, external_status: :unknown, external_id: 'u1')
    txn.stubs(:bot).returns(bot)
    bot.stubs(:get_order).returns(Result::Failure.new('EAPI:Rate limit exceeded'))
    relation = SolidQueue::Job.where(class_name: 'Bot::FetchAndUpdateOrderJob')

    freeze_time do
      relation.destroy_all
      Bot::FetchAndUpdateOrderJob.new(txn).perform_now
      assert_equal 15.seconds.from_now.to_i, relation.last.scheduled_at.to_i, 'attempt 1 → 15s'

      relation.destroy_all
      second = Bot::FetchAndUpdateOrderJob.new(txn)
      second.exception_executions['[Client::RateLimitedError]'] = 1
      second.perform_now
      assert_equal 30.seconds.from_now.to_i, relation.last.scheduled_at.to_i, 'attempt 2 → 30s'
    end
  end

  test 'raises a plain RuntimeError (not TransientNetworkError) for a non-transient Kraken failure' do
    bot = create(:dca_single_asset, :started, exchange: create(:kraken_exchange))
    txn = create(:transaction, bot: bot, status: :submitted, external_status: :unknown, external_id: 'u1')
    txn.stubs(:bot).returns(bot)
    bot.stubs(:get_order).returns(Result::Failure.new('exchange down'))

    error = assert_raises(RuntimeError) { Bot::FetchAndUpdateOrderJob.new.perform(txn) }
    refute_kind_of Client::TransientNetworkError, error
    assert_match(/exchange down/, error.message)
  end

  test 'a Kraken transient failure does NOT short-circuit the stale not_found path' do
    # A not_found result for an old order is still abandoned — the transient check
    # sits AFTER the stale handling, and a not_found sentence is not a transient code.
    bot = create(:dca_single_asset, :started, exchange: create(:kraken_exchange))
    old = (Bot::StaleOrderResolver::STALE_ORDER_THRESHOLD + 1.day).ago
    txn = create(:transaction, bot: bot, status: :submitted, external_status: :unknown,
                               external_id: 'TXID-STALE', created_at: old)
    txn.stubs(:bot).returns(bot)
    bot.stubs(:get_order).returns(Result::Failure.new('Kraken did not return data', data: { not_found: true }))

    assert_nothing_raised { Bot::FetchAndUpdateOrderJob.new.perform(txn) }
    assert_equal 'abandoned', txn.reload.external_status
  end

  test 'declares retry_on Client::TransientNetworkError' do
    handler_classes = Bot::FetchAndUpdateOrderJob.rescue_handlers.map(&:first)
    assert_includes handler_classes, 'Client::TransientNetworkError'
  end

  test 'enqueues on the per-exchange queue from its Transaction argument' do
    # Documents that the job's first arg is a Transaction (not a Bot); its exchange
    # delegates so retries re-enqueue on the right per-exchange queue.
    bot = create(:dca_single_asset, :started, exchange: create(:kraken_exchange))
    txn = create(:transaction, bot: bot, external_id: 'u1')

    assert_equal :kraken, Bot::FetchAndUpdateOrderJob.new(txn).queue_name
  end

  # W2b: a proxy/network timeout (any exchange) must become a retryable Client::TransientNetworkError,
  # not a terminal "Failed to fetch order" RuntimeError — so it retries instead of dropping the poll.
  test 'a network timeout raises TransientNetworkError (retried), not a bare RuntimeError' do
    bot = create(:dca_single_asset, :started) # binance — no exchange-specific :transient patterns
    txn = create(:transaction, bot: bot, status: :submitted, external_status: :unknown, external_id: 'u1')
    txn.stubs(:bot).returns(bot)
    bot.stubs(:get_order).returns(Result::Failure.new('Net::ReadTimeout with #<TCPSocket:(closed)>'))

    assert_raises(Client::TransientNetworkError) { Bot::FetchAndUpdateOrderJob.new.perform(txn) }
    assert Transaction.exists?(txn.id), 'the durable row must survive for the next sweep'
  end
end
