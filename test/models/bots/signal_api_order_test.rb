require 'test_helper'

# An order the caller sized itself, placed through a signal bot. It must end up exactly where a
# rule's order does — a row, a confirmation job, the same failure classification — and it must tell
# the caller what happened, because unlike a webhook sender this caller is still on the line.
class Bots::SignalApiOrderTest < ActiveSupport::TestCase
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
  end

  def last_transaction
    @bot.transactions.order(:created_at, :id).last
  end

  test 'a quote-sized buy is recorded as a submitted order on the bot' do
    @exchange.expects(:market_buy).with(has_entries(ticker: @ticker)).returns(Result::Success.new(order_id: 'api-1'))

    outcome = @bot.execute_api_order(side: :buy, amount: 100.to_d, amount_type: :quote)

    assert_equal :submitted, outcome.status
    assert_equal 'api-1', outcome.order_id
    txn = last_transaction
    assert_equal txn, outcome.transaction
    assert_predicate txn, :submitted?
    assert_predicate txn, :buy?
    assert_predicate txn, :market_order?
    assert_equal 'unknown', txn.external_status
    assert_equal 'REGULAR', txn.transaction_type
    assert_in_delta 100, txn.quote_amount.to_f, 1e-6
    assert_in_delta 100.0 / 50_000, txn.amount.to_f, 1e-9
    assert_in_delta 50_000, txn.price.to_f, 1e-6
  end

  test 'a base-sized sell is priced at the bid and submitted in base' do
    @exchange.expects(:market_sell).with(has_entries(amount_type: :base)).returns(Result::Success.new(order_id: 'api-2'))

    outcome = @bot.execute_api_order(side: :sell, amount: '0.5'.to_d, amount_type: :base)

    assert_equal :submitted, outcome.status
    txn = last_transaction
    assert_predicate txn, :sell?
    assert_in_delta 0.5, txn.amount.to_f, 1e-9
    assert_in_delta 24_500, txn.quote_amount.to_f, 1e-6
  end

  # The caller's number is the order: nothing is capped to the wallet, so nothing reads it.
  test 'no balance is read' do
    @exchange.expects(:get_balances).never
    @exchange.expects(:get_balance).never
    @exchange.stubs(:market_sell).returns(Result::Success.new(order_id: 'api-3'))

    @bot.execute_api_order(side: :sell, amount: 1.to_d, amount_type: :base)
  end

  test 'hands the accepted order to the confirmation job' do
    @exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'api-4'))

    assert_enqueued_with(job: Bot::FetchAndUpdateOrderJob) do
      @bot.execute_api_order(side: :buy, amount: 100.to_d, amount_type: :quote)
    end
  end

  test 'an order below the venue minimum is a skipped row and never sent' do
    @exchange.expects(:market_buy).never # the ticker's minimum_quote_size is 10

    outcome = @bot.execute_api_order(side: :buy, amount: 1.to_d, amount_type: :quote)

    assert_equal :skipped, outcome.status
    assert_predicate last_transaction, :skipped?
    assert_equal 1, @bot.bot_activity_logs.where(event: 'order_skipped').count
  end

  test 'a rejection is a failed row, reported once, with the venue message handed back' do
    @exchange.stubs(:market_buy).returns(Result::Failure.new('Account has insufficient balance for requested action.'))
    @bot.expects(:notify_about_error).once

    outcome = @bot.execute_api_order(side: :buy, amount: 100.to_d, amount_type: :quote)

    assert_equal :failed, outcome.status
    assert_includes outcome.errors, 'Account has insufficient balance for requested action.'
    assert_predicate last_transaction, :failed?
    assert_equal last_transaction, outcome.transaction
  end

  test 'a price that cannot be read is a failed row and nothing is sent' do
    stub_ticker_price_failure(@ticker, error: 'Price fetch failed')
    @exchange.expects(:market_buy).never

    outcome = @bot.execute_api_order(side: :buy, amount: 100.to_d, amount_type: :quote)

    assert_equal :failed, outcome.status
    assert_predicate last_transaction, :failed?
  end

  # The request may have landed. Neither a retry (buys twice) nor a failed row (may be false) is
  # honest, and the caller must be told it is unknown rather than that it failed.
  test 'an ambiguous placement leaves no row and says so' do
    @exchange.stubs(:market_buy).raises(Client::AmbiguousPlacementError, 'Net::ReadTimeout')
    @bot.expects(:notify_about_error).never

    outcome = @bot.execute_api_order(side: :buy, amount: 100.to_d, amount_type: :quote)

    assert_equal :ambiguous, outcome.status
    assert_includes outcome.errors.to_sentence, 'Net::ReadTimeout'
    assert_nil last_transaction
    assert_equal 1, @bot.bot_activity_logs.where(event: 'placement_ambiguous').count
  end

  test 'a wash-sale lock refuses the buy before the venue is read' do
    @bot.user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
    WashSaleLock.create!(user: @bot.user, asset_id: @ticker.base_asset_id, buy_locked_until: 10.days.from_now)
    @exchange.expects(:market_buy).never

    outcome = @bot.execute_api_order(side: :buy, amount: 100.to_d, amount_type: :quote)

    assert_equal :locked, outcome.status
    assert_nil last_transaction
  end

  # The denomination is the caller's wherever the venue takes it, as on an order without a bot.
  # Bots that derive both numbers from one let the venue's amount logic pick; here that would
  # turn "spend 100" into a base amount at a price that may have moved. (The test venue takes
  # either denomination on a market buy.)
  test 'a buy reaches the venue in the denomination it was sent in' do
    { quote: 100.to_d, base: '0.01'.to_d }.each do |amount_type, amount|
      @exchange.expects(:market_buy).with(ticker: @ticker, amount: amount, amount_type: amount_type)
               .returns(Result::Success.new(order_id: "api-buy-#{amount_type}"))

      assert_equal :submitted, @bot.execute_api_order(side: :buy, amount: amount, amount_type: amount_type).status
    end
  end

  # Every bot in the app sizes a sell in base, and some venues take nothing else (one raises on a
  # quote-sized sell before any request leaves). A quote-sized sell is converted at the bid.
  test 'a sell is always submitted in base' do
    @exchange.expects(:market_sell).with(ticker: @ticker, amount: '0.5'.to_d, amount_type: :base)
             .returns(Result::Success.new(order_id: 'api-sell-base'))
    assert_equal :submitted, @bot.execute_api_order(side: :sell, amount: '0.5'.to_d, amount_type: :base).status

    @exchange.expects(:market_sell).with(ticker: @ticker, amount: 98.to_d / 49_000, amount_type: :base)
             .returns(Result::Success.new(order_id: 'api-sell-quote'))
    assert_equal :submitted, @bot.execute_api_order(side: :sell, amount: 98.to_d, amount_type: :quote).status
  end

  # A venue that trades whole shares takes base only; one that trades notional takes quote only.
  # Sending the other denomination fails inside the adapter, so it is converted at the current
  # price — the same conversion every scheduled bot on that venue already gets.
  test 'a buy is converted where the venue takes only the other denomination' do
    @exchange.stubs(:minimum_amount_logic).returns(:base)
    @exchange.expects(:market_buy).with(ticker: @ticker, amount: 100.to_d / 50_000, amount_type: :base)
             .returns(Result::Success.new(order_id: 'api-base-only'))
    assert_equal :submitted, @bot.execute_api_order(side: :buy, amount: 100.to_d, amount_type: :quote).status

    @exchange.stubs(:minimum_amount_logic).returns(:quote)
    @exchange.expects(:market_buy).with(ticker: @ticker, amount: '0.01'.to_d * 50_000, amount_type: :quote)
             .returns(Result::Success.new(order_id: 'api-quote-only'))
    assert_equal :submitted, @bot.execute_api_order(side: :buy, amount: '0.01'.to_d, amount_type: :base).status
  end

  # Once the venue has accepted the order, nothing that goes wrong on our side may be reported as a
  # failure: the caller is told it was placed, with the id it can be reconciled by.
  test 'a confirmation that could not be queued still reports the order as placed' do
    @exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'api-acc'))
    Bot::FetchAndUpdateOrderJob.stubs(:perform_later).raises(SolidQueue::Job::EnqueueError, 'database is locked')
    @bot.expects(:notify_about_error).once

    outcome = @bot.execute_api_order(side: :buy, amount: 100.to_d, amount_type: :quote)

    assert_equal :submitted, outcome.status
    assert_equal 'api-acc', outcome.order_id
    assert_equal last_transaction, outcome.transaction
    assert_predicate last_transaction, :submitted?
  end

  test 'a callback raising after the row was committed still hands the row back' do
    @exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'api-cb'))
    AccountTransaction::SyncJob.stubs(:perform_later).raises(SolidQueue::Job::EnqueueError, 'database is locked')
    @bot.stubs(:notify_about_error)

    outcome = @bot.execute_api_order(side: :buy, amount: 100.to_d, amount_type: :quote)

    assert_equal :submitted, outcome.status
    assert_equal 'api-cb', outcome.order_id
    assert_equal 1, @bot.transactions.count
    assert_equal last_transaction, outcome.transaction
  end

  test 'a row that could not be written at all still reports the accepted order' do
    @exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'api-norow'))
    @bot.stubs(:persist_accepted_order!).raises(ActiveRecord::StatementInvalid, 'database is locked')
    @bot.stubs(:notify_about_error)

    outcome = @bot.execute_api_order(side: :buy, amount: 100.to_d, amount_type: :quote)

    assert_equal :submitted, outcome.status
    assert_equal 'api-norow', outcome.order_id
    assert_nil outcome.transaction
    assert_equal 'api-norow', @bot.bot_activity_logs.find_by(event: 'execution_failed').details['order_id']
  end

  test 'an acceptance with no order id is ambiguous, not placed' do
    @exchange.stubs(:market_buy).returns(Result::Success.new(order_id: nil))

    outcome = @bot.execute_api_order(side: :buy, amount: 100.to_d, amount_type: :quote)

    assert_equal :ambiguous, outcome.status
    assert_nil last_transaction
  end

  # Honeymaker venues hand a network failure on placement back as a Result, not an exception.
  test 'a network failure returned by the placement is ambiguous, not failed' do
    @exchange.stubs(:market_buy).returns(Result::Failure.new('Faraday::TimeoutError: Net::ReadTimeout'))
    @bot.expects(:notify_about_error).never

    outcome = @bot.execute_api_order(side: :buy, amount: 100.to_d, amount_type: :quote)

    assert_equal :ambiguous, outcome.status
    assert_nil last_transaction
  end

  # No job wraps this caller, so a raise before placement must come back as an answer, not escape.
  test 'a raise before placement is a failed row, never an exception' do
    @bot.stubs(:calculate_best_amount_info).raises(StandardError, 'minimums unreadable')
    @exchange.expects(:market_buy).never
    @bot.stubs(:notify_about_error)

    outcome = assert_nothing_raised { @bot.execute_api_order(side: :buy, amount: 100.to_d, amount_type: :quote) }

    assert_equal :failed, outcome.status
    assert_includes outcome.errors, 'minimums unreadable'
    assert_predicate last_transaction, :failed?
  end

  test 'a rule still reports what its order came to' do
    signal = create(:bot_signal, bot: @bot, direction: :buy, amount: 100, amount_type: :fixed)
    @exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'sig-1'))

    outcome = @bot.execute_signal(signal)

    assert_equal :submitted, outcome.status
    assert_equal 'sig-1', outcome.order_id
  end
end
