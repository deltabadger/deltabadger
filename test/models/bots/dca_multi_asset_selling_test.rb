require 'test_helper'

# DCA-out for a basket: each sell tick sells sell_quote_amount from the member furthest above its
# target weight, capped at what the bot holds (net of its own resting sells) and at what is free on
# the exchange. A member that cannot cover it hands the rest to the next-ranked one. No carry — an
# unsold remainder is dropped, as it is on the pair bot's sell leg.
class Bots::DcaMultiAssetSellingTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_multi_asset, user: create(:user)) # two members, 50/50
    @base0, @base1 = @bot.base_assets
    # ONE ticker array, pinned — the same idiom as dca_multi_asset_rebalance_test.rb.
    @tickers = @bot.composition_tickers
    @bot.instance_variable_set(:@tickers, @tickers)
    @bot.stubs(:composition_tickers).returns(@tickers)
    @ticker0 = @tickers.find { |ticker| ticker.base_asset_id == @base0.id }
    @ticker1 = @tickers.find { |ticker| ticker.base_asset_id == @base1.id }
    setup_bot_execution_mocks(@bot, price: 100)
    @bot.exchange.stubs(:market_sell).returns(*(1..4).map { |i| Result::Success.new(order_id: "s-#{i}") })
    @bot.exchange.expects(:market_buy).never
    Bot::FetchAndUpdateOrderJob.stubs(:perform_later)
    @bot.set_missed_quote_amount
    @bot.update!(direction: 'selling', sell_quote_amount: 100)
  end

  test 'a tick sells the whole amount from the most overweight member, and nothing else' do
    hold(base0: 700, base1: 300)

    sell_tick

    order = @bot.transactions.sole
    assert_equal ['sell', @base0.symbol, 'REGULAR'], [order.side, order.base, order.transaction_type]
    assert_in_delta 1.0, order.amount.to_f, 1e-9, '100 quote at a bid of 100'
  end

  test 'a balanced basket sells from its largest position, not its smallest' do
    # 700/300 on a 70/30 target is exactly balanced. Measured against the portfolio BEFORE the
    # withdrawal both read zero and the tie-break picks the smaller target; measured against what is
    # left after it, the larger position is the one above its share.
    weigh(base0: 0.7, base1: 0.3)
    hold(base0: 700, base1: 300)

    sell_tick

    assert_equal @base0.symbol, @bot.transactions.sole.base
  end

  # A member with no recorded weight is normalised to its current share, so it always reads exactly
  # on target. It is ranked like any other member at that share — it does not win the tie-break.
  test 'a small member with no recorded weight does not win the tie and get sold whole' do
    @bot.bot_index_assets.find_by(asset_id: @base0.id).update_columns(target_allocation: nil)
    hold(base0: 20, base1: 80)

    sell_tick(20)

    assert_equal @base1.symbol, @bot.transactions.sole.base
  end

  test 'a large member with no recorded weight is sold when it is the largest position, like any member' do
    @bot.bot_index_assets.find_by(asset_id: @base0.id).update_columns(target_allocation: nil)
    hold(base0: 80, base1: 20)

    sell_tick(20)

    assert_equal @base0.symbol, @bot.transactions.sole.base
  end

  test 'a member short on free balance sells what it has and the next member covers the rest' do
    hold(base0: 700, base1: 300, free: { base0: 0.4 })

    sell_tick

    sold = @bot.transactions.sell.order(:id).pluck(:base, :amount).map { |base, amount| [base, amount.to_f.round(8)] }
    assert_equal [[@base0.symbol, 0.4], [@base1.symbol, 0.6]], sold
  end

  test 'never sells more than the ledger holds net of its own resting sell, however full the wallet' do
    hold(base0: 50, base1: 0, free: { base0: 10 }) # ledger 0.5 units, wallet 10
    resting_sell(@base0, amount: 0.3)

    sell_tick

    assert_in_delta 0.2, placed.sole.amount.to_f, 1e-9
  end

  test 'a member whose resting sell already covers its excess is not picked again' do
    # 600/400 against 50/50 reads BTC overweight — but 300 of it is already on its way out.
    hold(base0: 600, base1: 400)
    resting_sell(@base0, amount: 3)

    sell_tick

    assert_equal @base1.symbol, placed.sole.base
  end

  test 'a member under a wash-sale lock is still sold: a lock forbids buying, not selling' do
    hold(base0: 700, base1: 300)
    @bot.stubs(:locked_asset_ids).returns([@base0.id])

    sell_tick

    assert_equal @base0.symbol, @bot.transactions.sole.base
  end

  test 'a loss sale is passed to the next member while that name has a resting buy anywhere on the account' do
    wash_sale_rule_on
    hold(base0: 700, base1: 300)
    @bot.stubs(:sell_at_loss?).returns(true)
    resting_buy_on_another_bot(@base0)

    sell_tick

    assert_equal @base1.symbol, placed_by_this_bot.sole.base
  end

  test 'a gain sale is not held back by a resting buy: it cannot be washed' do
    wash_sale_rule_on
    hold(base0: 700, base1: 300)
    @bot.stubs(:sell_at_loss?).returns(false)
    resting_buy_on_another_bot(@base0)

    sell_tick

    assert_equal @base0.symbol, placed_by_this_bot.sole.base
  end

  test 'a loss sale locks the name at placement, before any fill' do
    wash_sale_rule_on
    hold(base0: 700, base1: 300)
    @bot.stubs(:sell_at_loss?).returns(true)

    sell_tick

    assert_includes @bot.user.locked_asset_ids, @base0.id
  end

  test 'a placement the venue definitively refused puts the previous lock back' do
    wash_sale_rule_on
    hold(base0: 700, base1: 300)
    @bot.stubs(:sell_at_loss?).returns(true)
    @bot.exchange.unstub(:market_sell)
    @bot.exchange.stubs(:market_sell).returns(Result::Failure.new('Insufficient balance'))

    sell_tick

    assert_not_includes @bot.user.locked_asset_ids, @base0.id
  end

  # A returned failure is not proof nothing was placed (Exchange#ambiguous_placement_error?): the
  # sale may be on the book, so the name stays locked.
  { 'a gateway 504' => Result::Failure.new('HTTP 504', data: { status: 504 }),
    'a lost acknowledgement' => Result::Failure.new('no order id', data: { unacknowledged: true }) }.each do |label, failure|
    test "a placement that failed with #{label} keeps the lock: the sale may have landed" do
      wash_sale_rule_on
      hold(base0: 700, base1: 300)
      @bot.stubs(:sell_at_loss?).returns(true)
      @bot.exchange.unstub(:market_sell)
      @bot.exchange.stubs(:market_sell).returns(failure)

      sell_tick

      assert_includes @bot.user.locked_asset_ids, @base0.id
    end
  end

  test 'a later member failing cannot put back the lock an earlier member\'s sale took' do
    wash_sale_rule_on
    hold(base0: 700, base1: 300, free: { base0: 0.4 }) # base0 covers 40, base1 is asked for the rest
    @bot.stubs(:sell_at_loss?).returns(false)
    @bot.stubs(:sell_at_loss?).with(has_entry(ticker: @ticker0)).returns(true)
    @bot.exchange.unstub(:market_sell)
    @bot.exchange.stubs(:market_sell).returns(Result::Success.new(order_id: 's-1'),
                                              Result::Failure.new('Insufficient balance'))

    sell_tick

    assert_includes @bot.user.locked_asset_ids, @base0.id, 'the base0 sale went out; its lock stands'
  end

  test 'a placement that raised keeps the lock: the sale may have landed' do
    wash_sale_rule_on
    hold(base0: 700, base1: 300)
    @bot.stubs(:sell_at_loss?).returns(true)
    @bot.exchange.unstub(:market_sell)
    @bot.exchange.stubs(:market_sell).raises(Client::TransientNetworkError, 'read timeout')

    # Bot::ExchangeUser#with_placement_guard re-raises it as ambiguous, so no retry re-places it.
    assert_raises(Client::AmbiguousPlacementError) { sell_tick }
    assert_includes @bot.user.locked_asset_ids, @base0.id
  end

  test 'a sale is judged against the lots its own resting sells will not already have taken' do
    # FIFO: the resting sell takes the 50 lot when it fills, so the next unit sold is the 150 one — a
    # loss at 100, though the executed lots alone would call it a gain against the 50.
    wash_sale_rule_on
    filled_buy(@base0, amount: 1, cost: 50)
    filled_buy(@base0, amount: 1, cost: 150)
    resting_sell(@base0, amount: 1)

    assert @bot.sell_at_loss?(ticker: @ticker0, amount: 1.to_d, quote_amount: 100.to_d)
  end

  test 'a selling basket refuses to redeploy: buying the proceeds back is the opposite of selling' do
    assert_equal :selling, @bot.send(:redeploy_blocked_reason)
  end

  test 'a limit sell is priced above the last trade by the limit distance, and sized at that price' do
    hold(base0: 700, base1: 300)
    @bot.stubs(:limit_ordered?).returns(true)
    @bot.stubs(:limit_order_pcnt_distance_decimal).returns(0.01.to_d)
    @bot.exchange.stubs(:limit_sell).returns(Result::Success.new(order_id: 'l-1'))

    sell_tick

    order = @bot.transactions.sole
    assert_equal 'limit_order', order.order_type
    assert_in_delta 101, order.price.to_f, 1e-9
    assert_in_delta 100.0 / 101, order.amount.to_f, 1e-6
  end

  test 'a member whose venue floor is above the amount hands the sale to the next member' do
    hold(base0: 700, base1: 300)
    @ticker0.update!(minimum_quote_size: 1_000)

    sell_tick

    assert_equal @base1.symbol, @bot.transactions.sole.base
  end

  test 'when no member clears its floor, the tick writes one skipped row and places nothing' do
    hold(base0: 700, base1: 300)
    @tickers.each { |ticker| ticker.update!(minimum_quote_size: 1_000) }
    @bot.exchange.expects(:market_sell).never

    sell_tick

    assert_equal [[@base0.symbol, 'skipped']], @bot.transactions.pluck(:base, :status)
  end

  test 'a delisted member is never picked' do
    hold(base0: 700, base1: 300)
    @ticker0.update!(available: false)

    sell_tick

    assert_equal @base1.symbol, @bot.transactions.sole.base
  end

  test 'holdings it cannot value right now stop the tick as retryable, not as a dead bot' do
    hold(base0: 700, base1: 300, stale: true)

    assert_raises(Client::TransientNetworkError) { sell_tick }
    assert_empty @bot.transactions
  end

  test 'an emptied basket skips the tick, keeps running and makes no balance call' do
    hold(base0: 0, base1: 0)
    @bot.exchange.expects(:get_balances).never

    assert_predicate sell_tick, :success?
    assert_empty @bot.transactions
  end

  test 'a blank sell amount is a no-op that asks the exchange for nothing' do
    hold(base0: 700, base1: 300)
    @bot.set_missed_quote_amount
    @bot.update!(sell_quote_amount: nil)
    @bot.stubs(:refresh_composition).returns(Result::Success.new)
    @bot.exchange.expects(:get_balances).never

    assert_predicate @bot.execute_action, :success?
    assert_empty @bot.transactions
  end

  test 'a selling basket never spends its frozen buy carry' do
    hold(base0: 700, base1: 300)
    @bot.update_columns(transient_data: @bot.transient_data.merge('missed_quote_amount' => 250))
    @bot.stubs(:refresh_composition).returns(Result::Success.new)

    assert_predicate @bot.execute_action, :success?

    assert_empty @bot.transactions.buy
    assert_equal [@base0.symbol], @bot.transactions.sell.pluck(:base)
    assert_equal 250, @bot.reload.missed_quote_amount, 'the buy carry resumes intact on a flip back'
  end

  private

  def sell_tick(amount = 100) = @bot.set_orders(total_orders_amount_in_quote: amount.to_d, side: :sell)

  def placed = @bot.transactions.where.not(external_id: 'resting')

  def placed_by_this_bot = @bot.transactions.sell

  def filled_buy(asset, amount:, cost:)
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :closed,
                         external_id: "b-#{SecureRandom.hex(3)}", side: :buy, transaction_type: 'REGULAR',
                         base: asset.symbol, quote: @bot.quote_asset.symbol, price: cost.to_d / amount,
                         amount:, amount_exec: amount, quote_amount: cost, quote_amount_exec: cost)
  end

  def weigh(base0:, base1:)
    @bot.bot_index_assets.find_by(asset_id: @base0.id).update_columns(target_allocation: base0)
    @bot.bot_index_assets.find_by(asset_id: @base1.id).update_columns(target_allocation: base1)
  end

  def wash_sale_rule_on
    @bot.user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
  end

  # On a DIFFERENT bot of the same user: the lock is the taxpayer's, so any of their buys washes it.
  # The refresh the guard runs first is stubbed — it would ask the venue about this row.
  def resting_buy_on_another_bot(asset)
    other = create(:dca_multi_asset, user: @bot.user, exchange: @bot.exchange,
                                     base_assets: @bot.base_assets, quote_asset: @bot.quote_asset)
    create(:transaction, bot: other, exchange: other.exchange, status: :submitted, external_status: :open,
                         external_id: 'other-buy', side: :buy, transaction_type: 'REGULAR',
                         base: asset.symbol, quote: other.quote_asset.symbol, price: 100, amount: 1,
                         amount_exec: nil, quote_amount_exec: nil)
    Bot::FetchAndUpdateOpenOrdersJob.stubs(:perform_now)
  end

  def resting_sell(asset, amount:)
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :open,
                         external_id: 'resting', side: :sell, transaction_type: 'REGULAR',
                         base: asset.symbol, quote: @bot.quote_asset.symbol, price: 100, amount:,
                         amount_exec: nil, quote_amount_exec: nil)
  end

  # Values at a price of 100, so value / 100 units each.
  def hold(base0:, base1:, free: {}, stale: false)
    values = { @base0.symbol => base0, @base1.symbol => base1 }
    @bot.stubs(:metrics).returns(
      asset_breakdown: values.transform_values { |value| { amount: value.to_d / 100, quote_invested: value.to_d } }
    )
    @bot.stubs(:metrics_with_current_prices).returns(
      asset_values: values.transform_values { |value| { amount: value.to_d / 100, current_value: value.to_d } },
      asset_breakdown: values.transform_values { |value| { amount: value.to_d / 100 } },
      prices_stale: stale
    )
    stub_exchange_balances(@bot.exchange,
                           @bot.quote_asset_id => { free: 10_000, locked: 0 },
                           @base0.id => { free: free.fetch(:base0, 100), locked: 0 },
                           @base1.id => { free: free.fetch(:base1, 100), locked: 0 })
  end
end
