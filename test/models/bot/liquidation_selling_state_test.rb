require 'test_helper'

# "A sale is under way" — the marker the page reads, and the two properties that keep it safe:
# it never gates trading, and only the request that wrote it can take it down.
class Bot::LiquidationSellingStateTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_index, user: create(:user), with_api_key: true)
  end

  test 'nothing selling by default' do
    assert_not @bot.liquidation_selling?
    assert_not @bot.liquidation_selling?
  end

  test 'a mark reads as selling, and clearing takes it down' do
    token = @bot.mark_selling!

    assert @bot.liquidation_selling?
    assert @bot.clear_selling!(token)
    assert_not @bot.liquidation_selling?
  end

  test 'a marker nobody cleared expires' do
    @bot.mark_selling!
    travel(Bot::LiquidationState::SELLING_TTL + 1.minute) do
      assert_not @bot.reload.liquidation_selling?
    end
  end

  test 'a garbage marker reads as expired rather than raising' do
    # It is rendered inside a view; a stored value nobody expects must not take the page down.
    @bot.merge_transient_data!(Bot::LiquidationState::SELLING_KEY => 'not a hash')
    assert_nothing_raised { @bot.liquidation_selling? }
    assert_not @bot.liquidation_selling?
  end

  test 'a placement intent alone does NOT read as selling, so the way out stays open' do
    # A bare `placing` intent is indistinguishable from a worker that died before promoting it — and
    # what promotes it is the user pressing Sell again. If the spinner hid Sell on the strength of
    # that intent, the intent would keep the spinner alive forever and no Clear would appear either:
    # a deadlock no reload could break. It still blocks TRADING, which is the guard's job.
    @bot.start_liquidation_placement!('AAA')

    assert @bot.liquidation_pending?
    assert_not @bot.liquidation_selling?, 'Sell comes back, and clicking it promotes the stale intent'
    assert_equal :halted, @bot.send(:liquidation_blocked_reason), 'while trading stays blocked'
  end

  test 'an order left resting does NOT read as selling either, for the same reason' do
    # Bot::FetchAndUpdateOrderJob is one-shot and a stopped bot never sweeps, so a row can sit in
    # `waiting` long after the venue filled it. A further Sell is refused — but it sweeps on the way
    # through, which is how the row is recovered. A spinner would hide the only thing that fixes it.
    working_order('AAA')

    assert_not @bot.liquidation_selling?
    assert_equal :orders_waiting, @bot.send(:liquidation_blocked_reason), 'while trading stays blocked'
  end

  test 'a real placement spins, because its request left a marker' do
    @bot.mark_selling!
    @bot.start_liquidation_placement!('AAA')

    assert @bot.liquidation_selling?
  end

  test 'a stale owner cannot clear a newer request, so the spinner survives for the sale still coming' do
    # Separate instances on purpose: the danger is a job holding token A deleting the token that
    # request B has since written, which would drop the spinner while B's sale was still queued.
    first = Bot.find(@bot.id)
    token_a = first.mark_selling!
    second = Bot.find(@bot.id)
    token_b = second.mark_selling!

    assert_not first.clear_selling!(token_a), 'the losing owner clears nothing'
    assert Bot.find(@bot.id).liquidation_selling?, "B's marker survives A's clear"

    assert second.clear_selling!(token_b)
    assert_not Bot.find(@bot.id).liquidation_selling?
  end

  test 'a nil token clears nothing' do
    @bot.mark_selling!
    assert_not @bot.clear_selling!(nil)
    assert @bot.reload.liquidation_selling?
  end

  test 'the marker never gates trading' do
    # The whole reason it is safe to write this from a web request and clear it from an ensure: a
    # marker whose job died can only show a spinner, never stop the bot from selling again.
    @bot.mark_selling!

    assert_nil @bot.send(:liquidation_blocked_reason)
  end

  test 'the marker does not touch which way the bot trades' do
    # Bot#selling? is the direction predicate, read by the carry in Bot::Accountable among others.
    # The two must never be confused.
    before = @bot.pending_quote_amount
    @bot.mark_selling!

    assert_not @bot.selling?
    assert_equal before, @bot.pending_quote_amount
  end

  test 'a cold price cache skips the request-side repaint rather than blocking on the exchange' do
    # broadcast_metrics_panel renders metrics_with_current_prices, which on a cold cache goes and
    # asks the venue. One of its callers is a request that has just queued a trade.
    @bot.stubs(:metrics_with_current_prices_from_cache).returns(nil)
    @bot.expects(:broadcast_metrics_panel).never

    @bot.broadcast_selling_state(cached_only: true)
  end

  test 'a worker repaints even from a cold cache, having no request to hold up' do
    @bot.stubs(:metrics_with_current_prices_from_cache).returns(nil)
    @bot.expects(:broadcast_metrics_panel).once

    @bot.broadcast_selling_state
  end

  test 'a failing repaint is swallowed, never handed to the caller' do
    # Its callers are a request that has already queued a trade and an ensure block that must not
    # mask the job's own exception.
    @bot.stubs(:broadcast_metrics_panel).raises(RuntimeError, 'redis is down')

    assert_nothing_raised { @bot.broadcast_selling_state }
  end

  def working_order(base)
    Bot.any_instance.stubs(:broadcast_new_order)
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :open,
                         external_id: "open-#{base}", side: :sell, base: base, quote: @bot.quote_asset.symbol,
                         transaction_type: 'LIQUIDATION', price: 100, amount: 1)
  end
end
