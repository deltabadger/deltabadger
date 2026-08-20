require 'test_helper'

# Placement for the rebalance leg: what it sells, how much, priced from which side of the book, and
# how it is labelled. The state machine that sequences sell → buy lives in the resume test.
class Bots::DcaDualAssetRebalanceTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_dual_asset, user: create(:user))
    @order_id = setup_bot_execution_mocks(@bot, price: 100)
    @bot.exchange.stubs(:market_sell).returns(Result::Success.new(order_id: 'sell-1'))
    enable_rebalancing
  end

  test 'sells the overweight asset down to its target, not to the band edge' do
    # 70/30 against a 50/50 target on a 100-unit portfolio: 20 units of value are on the wrong side.
    stub_values(base0: 70, base1: 30)

    @bot.rebalance!

    order = @bot.transactions.last
    assert_equal 'sell', order.side
    assert_equal @bot.base0_asset.symbol, order.base
    assert_in_delta 20, order.quote_amount.to_f, 0.01
  end

  test 'sells the other asset when it is the overweight one' do
    stub_values(base0: 30, base1: 70)

    @bot.rebalance!

    assert_equal @bot.base1_asset.symbol, @bot.transactions.last.base
  end

  test 'rebalance orders are labelled REBALANCE, never REGULAR' do
    stub_values(base0: 70, base1: 30)

    @bot.rebalance!

    assert_equal 'REBALANCE', @bot.transactions.last.transaction_type
  end

  test 'a sell is capped by what is actually free on the exchange' do
    # The portfolio says 30 units of BTC are overweight, but only 0.05 is on the venue — the rest
    # is in cold storage. Allocation counts it; the sell cannot.
    stub_values(base0: 70, base1: 30)
    stub_exchange_balances(@bot.exchange, @bot.base0_asset_id => { free: 0.15, locked: 0 },
                                          @bot.base1_asset_id => { free: 1.0, locked: 0 },
                                          @bot.quote_asset_id => { free: 10_000, locked: 0 })

    @bot.rebalance!

    assert_in_delta 0.15, @bot.transactions.last.amount.to_f, 0.0001
  end

  test 'a sell takes the bid, not the ask' do
    stub_values(base0: 70, base1: 30)
    stub_ticker_bid_price(@bot.ticker0, price: 90)
    stub_ticker_ask_price(@bot.ticker0, price: 110)

    @bot.rebalance!

    assert_equal 90, @bot.transactions.last.price.to_f, 'crossing the spread on a sell means the bid'
  end

  test 'FeeCutter does not apply — a rebalance is always a market order' do
    # FeeCutter is a DCA setting and its premise, that waiting is free, is false here. A rebalance
    # order's size comes from a snapshot of the allocation, so an order left resting executes
    # against a picture that has since changed — and it fills precisely when the drift got worse,
    # because a sell parked above the market only trades if the overweight asset kept rising.
    @bot.stubs(:limit_ordered?).returns(true)
    @bot.exchange.expects(:limit_sell).never
    stub_values(base0: 70, base1: 30)

    @bot.rebalance!

    assert_equal 'market_order', @bot.transactions.last.order_type
  end

  test 'a market sell is sized off the bid even with FeeCutter on' do
    @bot.stubs(:limit_ordered?).returns(true)
    stub_ticker_bid_price(@bot.ticker0, price: 90)
    stub_ticker_last_price(@bot.ticker0, price: 100)
    stub_values(base0: 70, base1: 30)

    @bot.rebalance!

    assert_equal 90, @bot.transactions.last.price.to_f, 'the last price is a FeeCutter notion'
  end

  test 'drift too small to trade places nothing at all' do
    # Below the venue minimum. Deliberately silent: this repeats every poll while the drift lasts,
    # so a row or a log line each time would be noise, not information.
    stub_values(base0: 50.5, base1: 49.5)
    update_settings(rebalance_threshold: 0.001)

    assert_no_difference -> { @bot.transactions.count } do
      @bot.rebalance!
    end
    assert_not @bot.reload.rebalance_pending?
  end

  test 'a bot that is not due places nothing' do
    stub_values(base0: 51, base1: 49)

    assert_no_difference -> { @bot.transactions.count } do
      @bot.rebalance!
    end
  end

  test 'placing the sell records intent before the order, so a crash cannot re-sell' do
    stub_values(base0: 70, base1: 30)

    @bot.rebalance!

    pending = @bot.reload.rebalance_pending
    assert_equal Bot::Rebalanceable::PHASE_SELLING, pending[:phase]
    assert_equal @bot.transactions.last.id, pending[:sell_transaction_id]
  end

  test 'an unclassified placement failure halts as ambiguous instead of unwinding' do
    # A generic failure is NOT proof that nothing was placed — a placement timeout is
    # indistinguishable from a filled order, and there is no idempotency key to settle it.
    stub_values(base0: 70, base1: 30)
    @bot.exchange.stubs(:market_sell).returns(Result::Failure.new('gateway timeout'))

    @bot.rebalance!

    assert @bot.reload.rebalance_ambiguous?
  end

  test 'a placement failure proven to be pre-trade unwinds cleanly' do
    stub_values(base0: 70, base1: 30)
    @bot.exchange.stubs(:market_sell).returns(Result::Failure.new('boom'))
    @bot.exchange.stubs(:placement_transient_error?).returns(true)

    @bot.rebalance!

    assert_not @bot.reload.rebalance_pending?, 'nothing reached the venue, so nothing is owed'
  end

  test 'an accepted order with no usable id halts as ambiguous' do
    stub_values(base0: 70, base1: 30)
    @bot.exchange.stubs(:market_sell).returns(Result::Success.new(order_id: nil))

    @bot.rebalance!

    assert @bot.reload.rebalance_ambiguous?
  end

  test 'a buy is sized at the price it will actually be submitted at' do
    # On a venue with no native market order the sizing price IS the submission price. Sizing at the
    # touch while submitting 1 % away asks for more quote than the balance the buy was capped to —
    # an insufficient-funds rejection, which is not a placement-safe error, so it halts the rule
    # permanently rather than retrying.
    @bot.exchange.stubs(:market_price_for).with(has_entries(side: :buy)).returns(Result::Success.new(101.to_d))
    @bot.exchange.stubs(:market_price_for).with(has_entries(side: :sell)).returns(Result::Success.new(99.to_d))
    stub_values(base0: 70, base1: 30)
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING, remaining_quote_amount: 20)

    @bot.rebalance!

    buy = @bot.transactions.where(side: :buy).last
    assert_in_delta 20, (buy.amount.to_f * buy.price.to_f), 0.01,
                    'notional at the submitted price must equal the cash on hand, not exceed it'
  end

  private

  def update_settings(**attrs)
    @bot.settings = @bot.settings.merge(attrs.transform_keys(&:to_s))
    @bot.set_missed_quote_amount
    @bot.save!
  end

  def enable_rebalancing
    update_settings(rebalance_enabled: true, rebalance_threshold: 0.05)
  end

  def stub_values(base0:, base1:)
    @bot.stubs(:metrics_with_current_prices).returns(
      total_base0_amount_value_in_quote: base0.to_d,
      total_base1_amount_value_in_quote: base1.to_d,
      prices_stale: false
    )
  end
end
