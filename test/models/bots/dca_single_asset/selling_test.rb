require 'test_helper'

# M2 — Sell execution path for Bots::DcaSingleAsset (DCA-out).
# A selling bot sells a fixed base amount per period, capped to what it accumulated and to
# the live free balance, at the bid (market) or last*(1+distance) (limit). It never touches
# the buy-side missed-quote carry.
class Bots::DcaSingleAsset::SellingTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

  # A selling bot with `holdings` net executed base and a stubbed live free balance.
  def selling_bot(sell_amount: 0.5, holdings: 1.0, free_base: 1.0, limit: false)
    bot = create(:dca_single_asset, :started)
    bot.settings['limit_ordered'] = limit
    bot.direction = 'selling'
    bot.sell_amount = sell_amount
    bot.set_missed_quote_amount
    bot.save!
    if holdings.positive?
      create(:transaction, bot: bot, side: :buy, status: :submitted, external_status: :closed,
                           amount_exec: holdings, quote_amount_exec: holdings * 100)
    end
    bot.exchange.stubs(:get_balance).returns(Result::Success.new({ free: free_base.to_d, total: free_base.to_d }))
    bot
  end

  # == Market sell ==

  test 'a selling bot places a market SELL of the configured base amount at the bid price' do
    bot = selling_bot(sell_amount: 0.3, holdings: 1.0, free_base: 1.0)
    bot.ticker.stubs(:get_bid_price).returns(Result::Success.new(200.to_d))
    bot.exchange.expects(:market_sell)
       .with(has_entries(amount_type: :base))
       .returns(Result::Success.new({ order_id: 'sell-1' }))

    result = bot.set_order(side: :sell)

    assert_predicate result, :success?
    txn = bot.transactions.order(:created_at).last
    assert_predicate txn, :sell?
    assert_in_delta 0.3, txn.amount.to_f, 1e-6
    assert_in_delta 200, txn.price.to_f, 1e-6
    assert_in_delta 60, txn.quote_amount.to_f, 1e-6 # 0.3 * 200
  end

  test 'a market sell uses the bid price, not the ask' do
    bot = selling_bot(sell_amount: 0.3)
    bot.ticker.expects(:get_bid_price).returns(Result::Success.new(200.to_d))
    bot.ticker.expects(:get_ask_price).never
    bot.exchange.stubs(:market_sell).returns(Result::Success.new({ order_id: 'x' }))

    bot.set_order(side: :sell)
  end

  # == Limit sell ==

  test 'a limit sell prices at last * (1 + distance), above the last trade' do
    bot = selling_bot(sell_amount: 0.3, limit: true)
    bot.ticker.stubs(:get_last_price).returns(Result::Success.new(100_000.to_d))
    captured = nil
    bot.exchange.stubs(:limit_sell).with do |args|
      captured = args
      true
    end.returns(Result::Success.new({ order_id: 'l1' }))

    bot.set_order(side: :sell)

    # default limit_order_pcnt_distance is 0.001 → 100_000 * 1.001 = 100_100 (above last, unlike a buy)
    assert_in_delta 100_100, captured[:price].to_f, 1e-6
  end

  # == Sell amount cap ==

  test 'a selling bot may sell BEYOND what it accumulated (liquidate from the wallet)' do
    # holdings (0.5) no longer caps the sell — the bot can liquidate the whole wallet up to the
    # configured amount and the live free balance.
    bot = selling_bot(sell_amount: 2.0, holdings: 0.5, free_base: 10.0)
    bot.ticker.stubs(:get_bid_price).returns(Result::Success.new(100.to_d))
    bot.exchange.stubs(:market_sell).returns(Result::Success.new({ order_id: 'wallet' }))

    bot.set_order(side: :sell)

    assert_in_delta 2.0, bot.transactions.order(:created_at).last.amount.to_f, 1e-6
  end

  test 'a bot with no accumulated holdings still sells from the wallet' do
    bot = selling_bot(sell_amount: 0.5, holdings: 0.0, free_base: 5.0)
    bot.ticker.stubs(:get_bid_price).returns(Result::Success.new(100.to_d))
    bot.exchange.stubs(:market_sell).returns(Result::Success.new({ order_id: 'liq' }))

    bot.set_order(side: :sell)

    assert_in_delta 0.5, bot.transactions.order(:created_at).last.amount.to_f, 1e-6
  end

  test 'the sell is capped to the live free base balance (never exchange-rejected)' do
    bot = selling_bot(sell_amount: 2.0, holdings: 10.0, free_base: 0.4)
    bot.ticker.stubs(:get_bid_price).returns(Result::Success.new(100.to_d))
    bot.exchange.stubs(:market_sell).returns(Result::Success.new({ order_id: 'freecap' }))

    bot.set_order(side: :sell)

    assert_in_delta 0.4, bot.transactions.order(:created_at).last.amount.to_f, 1e-6
  end

  test 'a blank sell amount is a no-op skip (nothing to sell yet), not an error' do
    bot = selling_bot(sell_amount: nil, holdings: 1.0)
    bot.exchange.expects(:market_sell).never

    assert_no_difference -> { bot.transactions.count } do
      result = bot.set_order(side: :sell)
      assert_predicate result, :success?
    end
  end

  test 'a zero free balance is a no-op skip (nothing available to sell)' do
    bot = selling_bot(sell_amount: 0.5, holdings: 0.0, free_base: 0.0)
    bot.exchange.expects(:market_sell).never

    assert_no_difference -> { bot.transactions.count } do
      result = bot.set_order(side: :sell)
      assert_predicate result, :success?
    end
  end

  test 'a sellable amount below the exchange minimum is skipped (bot keeps running)' do
    bot = selling_bot(sell_amount: 0.0000001, holdings: 1.0, free_base: 1.0)
    bot.ticker.stubs(:get_bid_price).returns(Result::Success.new(100.to_d))
    bot.exchange.expects(:market_sell).never

    result = bot.set_order(side: :sell)

    assert_predicate result, :success?
    txn = bot.transactions.order(:created_at).last
    assert_predicate txn, :skipped?
    assert_predicate txn, :sell?
  end

  # == execute_action direction routing ==

  test 'execute_action routes a selling bot through the sell path' do
    bot = selling_bot(sell_amount: 0.3)
    bot.expects(:set_order).with(has_entries(side: :sell, update_missed_quote_amount: false))
       .returns(Result::Success.new)

    bot.execute_action
  end

  test 'execute_action still routes a buying bot through the buy path (unchanged)' do
    bot = create(:dca_single_asset, :started)
    bot.expects(:set_order).with(has_entries(update_missed_quote_amount: true))
       .returns(Result::Success.new)

    bot.execute_action
  end

  # == Accountable carry stays buy-only (invariant 3) ==

  test 'pending_quote_amount freezes (returns the stored missed_quote_amount) while selling' do
    bot = create(:dca_single_asset, :started)
    bot.direction = 'selling'
    bot.missed_quote_amount = 42
    bot.missed_quote_amount_was_set = true
    bot.save!

    assert_equal 42.to_d, bot.pending_quote_amount
  end

  test 'set_missed_quote_amount is a no-op freeze while selling (buy carry survives the sell phase)' do
    bot = create(:dca_single_asset, :started)
    bot.direction = 'selling'
    bot.missed_quote_amount = 30
    bot.missed_quote_amount_was_set = true
    bot.save!

    bot.set_missed_quote_amount

    assert_equal 30.to_d, bot.missed_quote_amount
  end

  test 'a limit-paused selling bot keeps its frozen carry (started_at nil must not wipe it)' do
    bot = create(:dca_single_asset, :started)
    bot.direction = 'selling'
    bot.missed_quote_amount = 25
    bot.missed_quote_amount_was_set = true
    bot.save!
    bot.stubs(:started_at).returns(nil) # a sell-side limit pause makes the decorated started_at nil

    assert_equal 25.to_d, bot.pending_quote_amount, 'the buy carry must survive a limit-paused sell tick'
  end

  test 'a sell inside the buy-carry window does not count as invested quote (does not shrink the next buy)' do
    freeze_time do
      bot = create(:dca_single_asset, :started) # buying
      bot.set_missed_quote_amount
      bot.save!
      baseline = bot.pending_quote_amount # no transactions yet
      calc_since = [bot.started_at, bot.settings_changed_at].compact.max
      # a sell whose proceeds must NOT be treated as invested quote on the buy side
      create(:transaction, bot: bot, side: :sell, status: :submitted, external_status: :closed,
                           external_id: 's1', amount: 1, quote_amount: 999, quote_amount_exec: 999,
                           created_at: calc_since + 1.second)
      bot.reload

      assert_equal baseline, bot.pending_quote_amount,
                   'a sell is divestment — it must not reduce the buy carry'
    end
  end

  # == Fundable bypass while selling ==

  test 'the end-of-funds notification is skipped while selling' do
    bot = create(:dca_single_asset, :started)
    bot.direction = 'selling'
    bot.set_missed_quote_amount
    bot.save!
    bot.stubs(:set_order).returns(Result::Success.new)
    bot.stubs(:funds_are_low?).returns(true)
    bot.stubs(:broadcast_below_minimums_warning)

    bot.expects(:notify_end_of_funds).never

    bot.execute_action
  end

  # == Sell-path failure handling (hardening) ==
  # A balance/price read failure must NOT be swallowed into a silent "nothing to sell" skip — it has
  # to surface so Bot::ActionJob retries (transient/throttle) or records execution_failed (other).

  test 'a transient balance-read failure surfaces for retry instead of silently skipping the sell' do
    bot = selling_bot(sell_amount: 0.5, holdings: 1.0)
    bot.exchange.stubs(:get_balance).returns(Result::Failure.new('EAPI:Rate limit exceeded'))
    bot.exchange.stubs(:throttled_error?).returns(true)

    assert_raises(Client::RateLimitedError) { bot.send(:sellable_base_amount) }
  end

  test 'a non-transient balance-read failure raises (surfaced as an execution failure, not a skip)' do
    bot = selling_bot(sell_amount: 0.5, holdings: 1.0)
    bot.exchange.stubs(:get_balance).returns(Result::Failure.new('Invalid API key'))
    bot.exchange.stubs(:throttled_error?).returns(false)
    bot.exchange.stubs(:transient_error?).returns(false)

    assert_raises(StandardError) { bot.send(:sellable_base_amount) }
  end

  test 'a genuine zero free balance is still treated as nothing to sell' do
    bot = selling_bot(sell_amount: 0.5, holdings: 1.0, free_base: 0.0)

    assert_equal 0, bot.send(:sellable_base_amount)
  end

  test 'a transient market-price failure on a sell retries instead of leaving a permanent failed order' do
    bot = selling_bot(sell_amount: 0.5, holdings: 1.0)
    bot.ticker.stubs(:get_bid_price).returns(Result::Failure.new('EService:Internal error'))
    bot.exchange.stubs(:transient_error?).returns(true)

    assert_raises(Client::TransientNetworkError) { bot.set_order(side: :sell) }
    assert_equal 0, bot.transactions.failed.count, 'a transient price-read failure must not leave a permanent failed order'
  end

  # == Net holdings (bot_net_holdings source) ==

  test 'total_amount nets executed buys minus executed sells' do
    bot = create(:dca_single_asset, :started)
    create(:transaction, bot: bot, side: :buy, amount_exec: 1.0)
    create(:transaction, bot: bot, side: :buy, amount_exec: 0.5)
    create(:transaction, bot: bot, side: :sell, amount_exec: 0.4)

    assert_in_delta 1.1, bot.total_amount.to_f, 1e-9
  end

  test 'total_amount never goes negative' do
    bot = create(:dca_single_asset, :started)
    create(:transaction, bot: bot, side: :buy, amount_exec: 0.2)
    create(:transaction, bot: bot, side: :sell, amount_exec: 0.5)

    assert_equal 0, bot.total_amount
  end

  # Legacy rows never had amount_exec backfilled; like the metrics, fall back to the requested
  # amount so a long-running bot can still sell what it accumulated.
  test 'total_amount falls back to amount for legacy rows missing amount_exec' do
    bot = create(:dca_single_asset, :started)
    create(:transaction, bot: bot, side: :buy, amount_exec: nil, amount: 0.5)
    create(:transaction, bot: bot, side: :buy, amount_exec: 0.3, amount: 0.3)

    assert_in_delta 0.8, bot.total_amount.to_f, 1e-9
  end

  # An accepted-but-unfilled buy is NOT a holding yet: only CLOSED orders count, so a bot reversed
  # into selling can never sell base it merely has open (or just-cancelled) buy orders for. The
  # amount fallback must not leak a still-open/cancelled order's requested size into holdings.
  test 'total_amount counts only closed orders, excluding open and cancelled buys' do
    bot = create(:dca_single_asset, :started)
    create(:transaction, bot: bot, side: :buy, external_status: :closed, amount_exec: 1.0, external_id: 'c1')
    create(:transaction, bot: bot, side: :buy, external_status: :open, amount_exec: nil, amount: 5.0, external_id: 'o1')
    create(:transaction, bot: bot, side: :buy, external_status: :cancelled, amount_exec: nil, amount: 3.0, external_id: 'x1')

    assert_in_delta 1.0, bot.total_amount.to_f, 1e-9
  end
end
