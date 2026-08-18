require 'test_helper'

# The third sentence: "Sell BTC for 100 USD / Day". The tick is sized in QUOTE and converted to base
# at the same price the order is placed at; every existing sell ceiling still applies.
class Bots::DcaSingleAsset::QuoteDenominatedSellingTest < ActiveSupport::TestCase
  def quote_selling_bot(sell_quote_amount: 100, free_base: 10.0, limit: false)
    bot = create(:dca_single_asset, :started)
    bot.settings['limit_ordered'] = limit
    bot.direction = 'selling'
    bot.sell_denomination = 'quote'
    bot.sell_quote_amount = sell_quote_amount
    bot.set_missed_quote_amount
    bot.save!
    bot.exchange.stubs(:get_balance).returns(Result::Success.new({ free: free_base.to_d, total: free_base.to_d }))
    bot
  end

  test 'a quote-denominated market sell sizes the base amount as quote / bid price' do
    bot = quote_selling_bot(sell_quote_amount: 100)
    bot.ticker.stubs(:get_bid_price).returns(Result::Success.new(200.to_d))
    bot.exchange.expects(:market_sell).with(has_entries(amount_type: :base))
       .returns(Result::Success.new({ order_id: 'q-1' }))

    result = bot.set_order(side: :sell)

    assert_predicate result, :success?
    txn = bot.transactions.order(:created_at).last
    assert_predicate txn, :sell?
    assert_in_delta 0.5, txn.amount.to_f, 1e-6 # 100 / 200
    assert_in_delta 100, txn.quote_amount.to_f, 1e-6
  end

  test 'a quote-denominated limit sell sizes at the LIMIT price, not the last trade' do
    # The whole point of fetching the price once: sizing at the bid and placing at last*(1+distance)
    # would not deliver the quote amount the sentence promises.
    bot = quote_selling_bot(sell_quote_amount: 100_100, limit: true)
    bot.ticker.stubs(:get_last_price).returns(Result::Success.new(100_000.to_d))
    bot.ticker.expects(:get_bid_price).never
    captured = nil
    bot.exchange.stubs(:limit_sell).with do |args|
      captured = args
      true
    end.returns(Result::Success.new({ order_id: 'q-l1' }))

    bot.set_order(side: :sell)

    assert_in_delta 100_100, captured[:price].to_f, 1e-6
    assert_in_delta 1.0, captured[:amount].to_f, 1e-6 # 100_100 / 100_100, not 100_100 / 100_000
  end

  test 'the live free base balance still caps a quote-denominated sell' do
    bot = quote_selling_bot(sell_quote_amount: 1000, free_base: 0.25)
    bot.ticker.stubs(:get_bid_price).returns(Result::Success.new(100.to_d))
    bot.exchange.stubs(:market_sell).returns(Result::Success.new({ order_id: 'q-2' }))

    bot.set_order(side: :sell)

    assert_in_delta 0.25, bot.transactions.order(:created_at).last.amount.to_f, 1e-6
  end

  test 'the base cap still caps a quote-denominated sell' do
    bot = quote_selling_bot(sell_quote_amount: 1000, free_base: 10.0)
    bot.settings['base_amount_limited'] = true
    bot.settings['base_amount_limit'] = 0.4
    bot.set_missed_quote_amount
    bot.save!
    bot.ticker.stubs(:get_bid_price).returns(Result::Success.new(100.to_d))
    bot.exchange.stubs(:market_sell).returns(Result::Success.new({ order_id: 'q-3' }))

    bot.set_order(side: :sell)

    assert_in_delta 0.4, bot.transactions.order(:created_at).last.amount.to_f, 1e-6
  end

  test 'a blank sell_quote_amount skips the tick without touching the exchange' do
    bot = quote_selling_bot(sell_quote_amount: nil)
    bot.ticker.expects(:get_bid_price).never
    bot.ticker.expects(:get_last_price).never
    bot.exchange.expects(:get_balance).never
    bot.exchange.expects(:market_sell).never

    assert_predicate bot.set_order(side: :sell), :success?
    assert_equal 'unconfigured_sell_amount', bot.send(:sell_skip_reason)
  end

  test 'a base-denominated sell makes no early price call (the base path is unchanged)' do
    bot = create(:dca_single_asset, :started)
    bot.direction = 'selling'
    bot.sell_amount = 0.3
    bot.set_missed_quote_amount
    bot.save!
    bot.exchange.stubs(:get_balance).returns(Result::Success.new({ free: 1.to_d, total: 1.to_d }))
    bot.ticker.expects(:get_bid_price).once.returns(Result::Success.new(100.to_d))
    bot.exchange.stubs(:market_sell).returns(Result::Success.new({ order_id: 'b-1' }))

    bot.set_order(side: :sell)
  end

  test 'a non-transient price failure returns before the balance is read' do
    bot = quote_selling_bot(sell_quote_amount: 100)
    bot.ticker.stubs(:get_bid_price).returns(Result::Failure.new(['Symbol not found']))
    bot.exchange.expects(:get_balance).never
    bot.exchange.expects(:market_sell).never

    assert_predicate bot.set_order(side: :sell), :failure?
    assert_predicate bot.transactions.order(:created_at).last, :failed?
  end

  test 'a throttled price read raises the typed error Bot::ActionJob retries on' do
    bot = quote_selling_bot(sell_quote_amount: 100)
    bot.ticker.stubs(:get_bid_price).returns(Result::Failure.new(['Too many requests']))
    bot.exchange.stubs(:throttled_error?).returns(true)

    assert_raises(Client::RateLimitedError) { bot.set_order(side: :sell) }
  end

  # == Smart Intervals is inert in quote mode ==

  test 'a smart-intervalled quote sell ignores the split and sells the full quote amount' do
    bot = quote_selling_bot(sell_quote_amount: 100)
    bot.settings['smart_intervaled'] = true
    bot.settings['smart_interval_quote_amount'] = 10.0
    bot.set_missed_quote_amount
    bot.save!
    bot.ticker.stubs(:get_bid_price).returns(Result::Success.new(100.to_d))
    bot.exchange.stubs(:market_sell).returns(Result::Success.new({ order_id: 'q-4' }))

    bot.set_order(side: :sell)

    assert_in_delta 1.0, bot.transactions.order(:created_at).last.amount.to_f, 1e-6
    assert_equal Automation::Schedulable::INTERVALS[bot.sell_interval], bot.effective_interval_duration
  end

  # == Pre-existing hole closed on the base side ==

  test 'a smart base sell with a cleared sell_amount no longer sells the stale split' do
    bot = create(:dca_single_asset, :started)
    bot.direction = 'selling'
    bot.sell_amount = 1.0
    bot.settings['smart_intervaled'] = true
    bot.settings['smart_interval_base_amount'] = 0.1
    bot.set_missed_quote_amount
    bot.save!

    bot.sell_amount = nil
    bot.set_missed_quote_amount
    bot.save!

    bot.ticker.expects(:get_bid_price).never
    bot.exchange.expects(:market_sell).never
    assert_predicate bot.set_order(side: :sell), :success?
  end
end
