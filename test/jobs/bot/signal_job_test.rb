require 'test_helper'

# One webhook call = one market order, sized by the rule that was hit. There is no schedule to fall
# back on: what this job does not place, nobody places — so every outcome leaves a row the user can
# see (a submitted, skipped or failed transaction, or an activity line).
class Bot::SignalJobTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper

  def queue_adapter_for_test
    ActiveJob::QueueAdapters::TestAdapter.new
  end

  setup do
    @bot = create(:signal_bot, :started)
    @exchange = @bot.exchange
    @ticker = @bot.ticker
    @exchange.stubs(:sleep) # Exchange#with_transient_retry backs off between read attempts
    stub_ticker_ask_price(@ticker, price: 50_000)
    stub_ticker_bid_price(@ticker, price: 49_000)
    stub_exchange_balances(@exchange, { @bot.quote_asset_id => { free: 10_000, locked: 0 },
                                        @bot.base_asset_id => { free: 2.0, locked: 0 } })
  end

  def rule(**attrs)
    create(:bot_signal, { bot: @bot, direction: :buy, amount: 100, amount_type: :fixed }.merge(attrs))
  end

  def perform(signal, triggered_at: Time.current)
    Bot::SignalJob.new.perform(@bot, signal, triggered_at)
  end

  def last_transaction
    @bot.transactions.order(:created_at, :id).last
  end

  # --- sizing --------------------------------------------------------------------------------

  test 'a fixed buy spends the rule amount of quote at the ask' do
    signal = rule(direction: :buy, amount: 100)
    @exchange.expects(:market_buy).with(has_entries(ticker: @ticker)).returns(Result::Success.new(order_id: 'sig-1'))
    @exchange.expects(:market_sell).never

    perform(signal)

    txn = last_transaction
    assert_predicate txn, :submitted?
    assert_equal 'unknown', txn.external_status
    assert_equal 'sig-1', txn.external_id
    assert_predicate txn, :buy?
    assert_predicate txn, :market_order?
    assert_in_delta 100, txn.quote_amount.to_f, 1e-6
    assert_in_delta 100.0 / 50_000, txn.amount.to_f, 1e-9
    assert_in_delta 50_000, txn.price.to_f, 1e-6
  end

  # Hyperliquid and Gemini emulate a market order with a limit that crosses the spread; the exchange
  # knows that price, and sizing must use it or the order asks for more quote than the rule says.
  test 'sizes at the price a market order would execute at, not the raw touch' do
    signal = rule(direction: :buy, amount: 100)
    @exchange.stubs(:market_price_for).with(ticker: @ticker, side: :buy).returns(Result::Success.new(101.to_d))
    @exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'sig-x1'))

    perform(signal)

    txn = last_transaction
    assert_in_delta 101, txn.price.to_f, 1e-6
    assert_in_delta 100.0 / 101, txn.amount.to_f, 1e-9
    assert_in_delta 100, txn.quote_amount.to_f, 1e-6
  end

  test 'a fixed buy makes no balance call' do
    signal = rule(direction: :buy, amount: 100)
    @exchange.expects(:get_balances).never
    @exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'sig-2'))

    perform(signal)
  end

  test 'a percentage buy spends that share of the spendable quote balance' do
    signal = rule(direction: :buy, amount: 10, amount_type: :percentage)
    @exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'sig-3'))

    perform(signal)

    assert_in_delta 1_000, last_transaction.quote_amount.to_f, 1e-6 # 10% of 10_000
  end

  # On a margin venue what a bot can spend is buying power, not the settled balance; the exchange
  # decides which (Exchange#spendable_balance), exactly as it does for the DCA low-funds check.
  test 'a percentage buy reads the spendable balance, not the free one' do
    signal = rule(direction: :buy, amount: 50, amount_type: :percentage)
    @exchange.stubs(:spendable_balance).returns(400.to_d)
    @exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'sig-3b'))

    perform(signal)

    assert_in_delta 200, last_transaction.quote_amount.to_f, 1e-6
  end

  # "Sell for 9 800 USD": the rule is written in quote, the order is sized in base at the bid.
  test 'a fixed sell is quote-denominated: base = amount / bid' do
    signal = rule(direction: :sell, amount: 9_800)
    @exchange.expects(:market_sell).with(has_entries(amount_type: :base)).returns(Result::Success.new(order_id: 'sig-4'))
    @exchange.expects(:market_buy).never

    perform(signal)

    txn = last_transaction
    assert_predicate txn, :sell?
    assert_in_delta 0.2, txn.amount.to_f, 1e-9 # 9_800 / 49_000
    assert_in_delta 9_800, txn.quote_amount.to_f, 1e-6
    assert_in_delta 49_000, txn.price.to_f, 1e-6
  end

  test 'a fixed sell never sells more than the free base balance' do
    signal = rule(direction: :sell, amount: 1_000_000) # ~20 BTC worth; the wallet holds 2
    @exchange.stubs(:market_sell).returns(Result::Success.new(order_id: 'sig-5'))

    perform(signal)

    assert_in_delta 2.0, last_transaction.amount.to_f, 1e-9
  end

  test 'a percentage sell sells that share of the free base balance' do
    signal = rule(direction: :sell, amount: 25, amount_type: :percentage)
    @exchange.stubs(:market_sell).returns(Result::Success.new(order_id: 'sig-6'))

    perform(signal)

    assert_in_delta 0.5, last_transaction.amount.to_f, 1e-9
    assert_in_delta 0.5 * 49_000, last_transaction.quote_amount.to_f, 1e-6
  end

  test 'an order below the exchange minimum is recorded as skipped, not sent' do
    signal = rule(direction: :buy, amount: 1) # the ticker's minimum_quote_size is 10
    @exchange.expects(:market_buy).never

    perform(signal)

    assert_predicate last_transaction, :skipped?
    assert_equal 1, @bot.bot_activity_logs.where(event: 'order_skipped').count
  end

  test 'selling from an empty wallet is recorded as skipped' do
    stub_exchange_balances(@exchange, { @bot.base_asset_id => { free: 0, locked: 0 } })
    signal = rule(direction: :sell, amount: 50, amount_type: :percentage)
    @exchange.expects(:market_sell).never

    perform(signal)

    assert_predicate last_transaction, :skipped?
  end

  # --- what the order is, once placed ----------------------------------------------------------

  test 'hands the accepted order to the confirmation job' do
    signal = rule
    @exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'sig-7'))

    assert_enqueued_with(job: Bot::FetchAndUpdateOrderJob) { perform(signal) }
  end

  # Tax, tracker and contribution accounting all read REGULAR rows; a signal buy is a contribution
  # like any other.
  test 'a signal order is a regular transaction on the bot exchange' do
    signal = rule
    @exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'sig-8'))

    perform(signal)

    txn = last_transaction
    assert_equal 'REGULAR', txn.transaction_type
    assert_equal @exchange, txn.exchange
    assert_equal @bot.base_asset.symbol, txn.base
    assert_equal @bot.quote_asset.symbol, txn.quote
  end

  # --- failures --------------------------------------------------------------------------------

  test 'an exchange rejection is recorded as a failed order and reported' do
    signal = rule
    @exchange.stubs(:market_buy).returns(Result::Failure.new('Account has insufficient balance for requested action.'))
    @bot.expects(:notify_about_error).once

    perform(signal)

    txn = last_transaction
    assert_predicate txn, :failed?
    assert_includes txn.error_messages, 'Account has insufficient balance for requested action.'
  end

  # A TradingView alert can call every 30 seconds against a broken key. One email says it broke;
  # the next would say the same thing.
  test 'a rule that keeps failing does not keep emailing' do
    signal = rule
    @exchange.stubs(:market_buy).returns(Result::Failure.new('Invalid API-key, IP, or permissions for action.'))
    @bot.expects(:notify_about_error).once

    3.times { perform(signal) }

    assert_equal 3, @bot.transactions.failed.count
  end

  test 'a failure after a success is reported again' do
    signal = rule
    @exchange.stubs(:market_buy).returns(Result::Failure.new('boom'))
             .then.returns(Result::Success.new(order_id: 'ok-1'))
             .then.returns(Result::Failure.new('boom'))
    @bot.expects(:notify_about_error).twice

    3.times { perform(signal) }
  end

  # A skip is not a recovery: fail, skip, fail is one breakage, not two.
  test 'a skipped order between two failures does not re-arm the email' do
    signal = rule
    @exchange.stubs(:market_buy).returns(Result::Failure.new('boom'))
    @bot.expects(:notify_about_error).once

    perform(signal)
    perform(rule(amount: 1)) # below minimum → skipped
    perform(signal)

    assert_equal 2, @bot.transactions.failed.count
    assert_equal 1, @bot.transactions.skipped.count
  end

  test 'a price that cannot be read is recorded as a failed order' do
    signal = rule
    stub_ticker_price_failure(@ticker, error: 'Price fetch failed')
    @exchange.expects(:market_buy).never

    perform(signal)

    assert_predicate last_transaction, :failed?
  end

  test 'a balance that cannot be read is recorded as a failed order' do
    @exchange.stubs(:get_balances).returns(Result::Failure.new('Invalid API-key, IP, or permissions for action.'))
    signal = rule(direction: :sell, amount: 50, amount_type: :percentage)
    @exchange.expects(:market_sell).never

    perform(signal)

    assert_predicate last_transaction, :failed?
  end

  # Reads are idempotent, so a blip is retried in place (Exchange#with_transient_retry) — there is
  # no next interval to carry a lost signal to. Honeymaker venues return the failure; Alpaca and
  # IBKR raise it. Both shapes retry, and both end in a failed row when they do not recover.
  test 'a transient price read that raises is retried in place' do
    signal = rule
    @ticker.stubs(:get_ask_price).raises(Client::TransientNetworkError, 'Net::OpenTimeout')
           .then.returns(Result::Success.new(50_000.to_d))
    @exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'sig-r0'))

    perform(signal)

    assert_predicate last_transaction, :submitted?
  end

  test 'a read that keeps raising is recorded as a failed order, not raised' do
    signal = rule
    @ticker.stubs(:get_ask_price).raises(Client::TransientNetworkError, 'Net::OpenTimeout')
    @exchange.expects(:market_buy).never

    perform(signal)

    assert_predicate last_transaction, :failed?
  end

  test 'a transient price read is retried in place' do
    signal = rule
    @ticker.stubs(:get_ask_price).returns(Result::Failure.new('Faraday::ConnectionFailed: execution expired'))
           .then.returns(Result::Success.new(50_000.to_d))
    @exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'sig-r1'))

    perform(signal)

    assert_predicate last_transaction, :submitted?
  end

  test 'a read that stays transient is recorded as a failed order, not raised' do
    signal = rule
    stub_ticker_price_failure(@ticker, error: 'Faraday::ConnectionFailed: execution expired')
    @exchange.expects(:market_buy).never

    perform(signal) # must not raise: a raise would re-run the job, and the job places orders

    assert_predicate last_transaction, :failed?
  end

  # A -1021 is a pre-trade rejection: the order never reached the book, so placing it again is safe
  # and a failed row would be a lie. Bot::ActionJob reschedules; this job re-places once.
  test 'a pre-trade timestamp rejection is re-placed once' do
    signal = rule
    @exchange.stubs(:market_buy).returns(Result::Failure.new('Timestamp for this request is outside of the recvWindow'))
             .then.returns(Result::Success.new(order_id: 'sig-t1'))

    perform(signal)

    assert_equal 1, @bot.transactions.count
    assert_predicate last_transaction, :submitted?
  end

  # The request may have landed after the response was lost. Neither a retry (buys twice) nor a
  # failed row (may be false) is honest; the activity line is the same one Bot::ActionJob writes.
  test 'an ambiguous placement is logged and not retried' do
    signal = rule
    @exchange.stubs(:market_buy).raises(Client::AmbiguousPlacementError, 'Net::ReadTimeout')
    @bot.expects(:notify_about_error).never

    perform(signal)

    assert_equal 1, @bot.bot_activity_logs.where(event: 'placement_ambiguous').count
    assert_nil last_transaction
  end

  # Honeymaker venues hand a network failure on placement back as a Result, not an exception, so
  # the placement guard never sees it. It is the same unknown outcome and gets the same answer.
  test 'a network failure returned by the placement is ambiguous, not failed' do
    signal = rule
    @exchange.stubs(:market_buy).returns(Result::Failure.new('Faraday::TimeoutError: Net::ReadTimeout'))
    @bot.expects(:notify_about_error).never

    perform(signal)

    assert_equal 1, @bot.bot_activity_logs.where(event: 'placement_ambiguous').count
    assert_nil last_transaction
  end

  # The app-level clients report an HTML-bodied gateway error as "HTTP 5xx" with the status in the
  # result's data; IBKR reports an acceptance whose id went missing in words. Neither is a rejection.
  test 'a gateway error on placement is ambiguous, not failed' do
    signal = rule
    @exchange.stubs(:market_buy).returns(Result::Failure.new('HTTP 504', data: { status: 504 }))
    @bot.expects(:notify_about_error).never

    perform(signal)

    assert_equal 1, @bot.bot_activity_logs.where(event: 'placement_ambiguous').count
    assert_nil last_transaction
  end

  test 'an acceptance the venue reported without an order id is ambiguous' do
    signal = rule
    @exchange.stubs(:market_buy).returns(Result::Failure.new('IBKR accepted the order but returned no order id for BTC',
                                                             data: { unacknowledged: true }))
             .then.returns(Result::Success.new(order_id: nil))
             .then.returns(Result::Success.new(order_id: 'BTCUSDT-')) # honeymaker's composed id, venue part missing
             .then.returns(Result::Failure.new('Hyperliquid order failed: no statuses returned', data: { unacknowledged: true }))
    @bot.expects(:notify_about_error).never

    4.times { perform(signal) }

    assert_equal 4, @bot.bot_activity_logs.where(event: 'placement_ambiguous').count
    assert_nil last_transaction
  end

  # The venue answered 200 and the body could not be read: an order with no acknowledgement.
  test 'an unreadable 2xx on placement is ambiguous' do
    signal = rule
    @exchange.stubs(:market_buy).returns(Result::Failure.new('Unreadable response (HTTP 200)', data: { status: 200, unreadable: true }))
    @bot.expects(:notify_about_error).never

    perform(signal)

    assert_equal 1, @bot.bot_activity_logs.where(event: 'placement_ambiguous').count
    assert_nil last_transaction
  end

  # A 4xx is the venue saying no. That one is a failed row.
  test 'a client-side rejection stays a failed order' do
    signal = rule
    @exchange.stubs(:market_buy).returns(Result::Failure.new('HTTP 401', data: { status: 401 }))
    @bot.expects(:notify_about_error).once

    perform(signal)

    assert_predicate last_transaction, :failed?
  end

  # A one-shot placement job has nothing an operator's Retry could safely do, so the failure is
  # not re-raised into a failed execution; the failed row, the email and the error log are the record.
  test 'an unexpected error before placement is recorded as a failed order and reported, not re-raised' do
    signal = rule
    @ticker.stubs(:get_ask_price).raises(RuntimeError, 'Wrong ask price')
    @exchange.expects(:market_buy).never
    @bot.expects(:notify_about_error).once

    assert_nothing_raised { 2.times { perform(signal) } }

    assert_equal 2, @bot.transactions.failed.count
    assert_includes last_transaction.error_messages, 'Wrong ask price'
  end

  # An exception thrown by the placement call itself has an unknown outcome: the request may have
  # gone out. It is ambiguous, not failed, for the same reason a timeout is.
  test 'an unexpected error inside the placement is ambiguous' do
    signal = rule
    @exchange.stubs(:market_buy).raises(RuntimeError, 'unexpected response shape')
    @bot.expects(:notify_about_error).never

    assert_nothing_raised { perform(signal) }

    assert_equal 1, @bot.bot_activity_logs.where(event: 'placement_ambiguous').count
    assert_nil last_transaction
  end

  # Once the venue has accepted the order, nothing that goes wrong on our side may be reported as a
  # failed order: the submitted row and its exchange id are what lets the fill be confirmed later.
  test 'a failure after the venue accepted the order keeps the submitted row and says so' do
    signal = rule
    @exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'sig-acc'))
    Bot::FetchAndUpdateOrderJob.stubs(:perform_later).raises(SolidQueue::Job::EnqueueError, 'database is locked')
    @bot.expects(:notify_about_error).once

    assert_nothing_raised { perform(signal) }

    assert_equal 1, @bot.transactions.count
    assert_predicate last_transaction, :submitted?
    assert_equal 'sig-acc', last_transaction.external_id
    failure = @bot.bot_activity_logs.find_by(event: 'execution_failed')
    assert_equal 'sig-acc', failure.details['order_id']
  end

  # Writing a transaction fires after-commit callbacks (a broadcast, the account-sync enqueue) that
  # can raise with the row already committed. Recording an outcome is its own boundary: a committed
  # row stays, the reporting failure is logged, and no second row is written for the same call.
  test 'a callback failing after a skipped row was written does not add a failed row' do
    signal = rule(amount: 1) # below minimum → skipped
    AccountTransaction::SyncJob.stubs(:perform_later).raises(SolidQueue::Job::EnqueueError, 'database is locked')
    @bot.expects(:notify_about_error).never

    assert_nothing_raised { perform(signal) }

    assert_equal 1, @bot.transactions.count
    assert_predicate last_transaction, :skipped?
  end

  test 'a callback failing after a failed row was written does not add a second one' do
    signal = rule
    @exchange.stubs(:market_buy).returns(Result::Failure.new('boom'))
    AccountTransaction::SyncJob.stubs(:perform_later).raises(SolidQueue::Job::EnqueueError, 'database is locked')
    @bot.expects(:notify_about_error).once

    assert_nothing_raised { perform(signal) }

    assert_equal 1, @bot.transactions.count
    assert_predicate last_transaction, :failed?
  end

  test 'a callback failing after the submitted row was written keeps it and says so' do
    signal = rule
    @exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'sig-cb'))
    AccountTransaction::SyncJob.stubs(:perform_later).raises(SolidQueue::Job::EnqueueError, 'database is locked')
    @bot.expects(:notify_about_error).once

    assert_nothing_raised { perform(signal) }

    assert_equal 1, @bot.transactions.count
    assert_predicate last_transaction, :submitted?
    assert_equal 'sig-cb', @bot.bot_activity_logs.find_by(event: 'execution_failed').details['order_id']
  end

  # perform_later answers false, not an exception, when the queue quietly refuses the row.
  test 'a confirmation the queue would not keep is reported like any post-acceptance failure' do
    signal = rule
    @exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'sig-nq'))
    Bot::FetchAndUpdateOrderJob.stubs(:perform_later).returns(false)
    @bot.expects(:notify_about_error).once

    assert_nothing_raised { perform(signal) }

    assert_predicate last_transaction, :submitted?
    assert_equal 'sig-nq', @bot.bot_activity_logs.find_by(event: 'execution_failed').details['order_id']
  end

  test 'a notification that cannot be sent does not become a second failed order' do
    signal = rule
    @exchange.stubs(:market_buy).returns(Result::Failure.new('boom'))
    @bot.stubs(:notify_about_error).raises(SolidQueue::Job::EnqueueError, 'database is locked')

    assert_nothing_raised { perform(signal) }

    assert_equal 1, @bot.transactions.failed.count
  end

  # --- gates: the claim was made in the controller; the world may have moved since ---------------

  # Accepted calls wait behind the per-exchange semaphore; a "buy now" that arrives ten minutes
  # late is a different trade. Dropped visibly — the user was told the call was accepted.
  test 'a call that waited too long in the queue is dropped and says so' do
    signal = rule
    @exchange.expects(:market_buy).never

    perform(signal, triggered_at: (Bot::SignalJob::MAX_AGE + 1.second).ago)

    assert_equal 1, @bot.bot_activity_logs.where(event: 'signal_expired').count
    assert_nil last_transaction
  end

  test 'a call still fresh at the edge of the window runs' do
    signal = rule
    @exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'sig-f1'))

    perform(signal, triggered_at: (Bot::SignalJob::MAX_AGE - 1.second).ago)

    assert_predicate last_transaction, :submitted?
  end

  test 'a bot stopped after the call was accepted places nothing and says so' do
    signal = rule
    @bot.update!(status: :stopped)
    @exchange.expects(:market_buy).never

    perform(signal)

    assert_equal 1, @bot.bot_activity_logs.where(event: 'signal_ignored').count
    assert_nil last_transaction
  end

  test 'a rule switched off after the call was accepted places nothing and says so' do
    signal = rule(enabled: false)
    @exchange.expects(:market_buy).never

    perform(signal)

    assert_equal 1, @bot.bot_activity_logs.where(event: 'signal_ignored').count
    assert_nil last_transaction
  end

  test 'a closed market is logged and nothing is placed' do
    signal = rule
    @exchange.stubs(:market_open?).returns(false)
    @exchange.expects(:market_buy).never

    perform(signal)

    assert_equal 1, @bot.bot_activity_logs.where(event: 'signal_market_closed').count
    assert_nil last_transaction
  end

  # An IBKR key registered but not yet activated must never reach a live IBKR call (Bot::ActionJob
  # reschedules in that state; a one-shot signal can only decline and say so).
  test 'a key still pending activation is logged and nothing is placed' do
    signal = rule
    ApiKey.any_instance.stubs(:pending_activation?).returns(true) # Automation::Dryable hands out a fresh key per call
    @exchange.expects(:market_buy).never

    perform(signal)

    assert_equal 1, @bot.bot_activity_logs.where(event: 'signal_api_key_pending').count
    assert_nil last_transaction
  end

  test 'authenticates the exchange with the bot key before trading' do
    signal = rule
    @bot.expects(:ensure_exchange_authenticated).at_least_once # once for the market check, once per venue call
    @exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'sig-9'))

    perform(signal)
  end

  # --- the job itself ----------------------------------------------------------------------------

  test 'runs on the exchange queue like every bot job' do
    assert_equal @exchange.name_id.to_s, Bot::SignalJob.new(@bot, rule, Time.current).queue_name.to_s
    assert_operator Bot::SignalJob, :<, BotJob
  end

  # Solid Queue keys the semaphore on [group, key]; without the group a signal would run against a
  # DCA tick on the same exchange account, both sizing off the same free balance
  # (Bot::RebalanceJob and Bot::LiquidateExitedJob join for the same reason).
  test 'shares the per-exchange semaphore with Bot::ActionJob' do
    assert_equal "Bot::ActionJob/exchange_#{@exchange.name_id}", Bot::SignalJob.new(@bot, rule, Time.current).concurrency_key
  end

  # A job-level retry around placement replays the placement — the known double-buy bug class.
  # Reads retry in place instead; a rule deleted in flight is discarded, not retried forever.
  test 'declares no retry_on, and discards a rule deleted before it ran' do
    handlers = Bot::SignalJob.rescue_handlers.map(&:first)

    assert_not_includes handlers, 'Client::TransientNetworkError'
    assert_not_includes handlers, 'Client::RateLimitedError'
    assert_not_includes handlers, 'Client::AmbiguousPlacementError'
    assert_includes handlers, 'ActiveJob::DeserializationError'
  end
end
