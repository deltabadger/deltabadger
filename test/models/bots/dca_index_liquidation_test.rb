require 'test_helper'

# Selling the assets an index has dropped. Everything here is about NOT trading when we should not:
# the guards, the skips, and the halt that follows a placement whose outcome we cannot know.
class Bots::DcaIndexLiquidationTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_index, user: create(:user), with_api_key: true)
    @assets = %w[AAA BBB CCC].to_h do |symbol|
      asset = create(:asset, symbol: symbol, name: "Coin #{symbol}", external_id: "coin-#{symbol.downcase}")
      ticker = create(:ticker, exchange: @bot.exchange, base_asset: asset, quote_asset: @bot.quote_asset)
      [symbol, { asset: asset, ticker: ticker }]
    end
    @bot.instance_variable_set(:@tickers, @assets.values.map { |a| a[:ticker] })
    @bot.stubs(:refresh_composition).returns(Result::Success.new)
  end

  # == which holdings are quitters ==

  test 'a holding the index no longer wants is a quitter' do
    index_membership('AAA', 'BBB')
    exited('CCC')
    stub_holdings('AAA' => 50, 'BBB' => 30, 'CCC' => 20)

    assert_equal(%w[CCC], @bot.exited_holdings.map { |h| h[:symbol] })
  end

  test 'a holding the index never knew about is a quitter too' do
    # Its BotIndexAsset row can predate a composition change, or be missing entirely.
    index_membership('AAA', 'BBB')
    stub_holdings('AAA' => 50, 'BBB' => 30, 'CCC' => 20)

    assert_equal(%w[CCC], @bot.exited_holdings.map { |h| h[:symbol] })
  end

  test 'an empty composition means nothing is a quitter, not that everything is' do
    # A bot whose first refresh has not landed knows no index. Reading that as "the index is empty"
    # would offer to liquidate the entire portfolio.
    stub_holdings('AAA' => 50, 'BBB' => 30)

    assert_empty @bot.exited_holdings
  end

  test 'the quitter row carries the figures the table renders' do
    index_membership('AAA')
    exited('CCC')
    stub_holdings('AAA' => 50, 'CCC' => 20)

    row = @bot.exited_holdings.first
    assert_equal 'CCC', row[:symbol]
    assert_in_delta 20, row[:current_value].to_f, 0.0001
    assert_equal @assets['CCC'][:ticker], row[:ticker]
  end

  # Selling rounds the base amount down to the venue's precision, so a holding with more decimals
  # than the venue trades in always leaves a remainder. It is `positive?`, so the row used to sit
  # under "Left the index" forever showing 0.00 over a Sell button that could never clear it.
  test 'a dust remainder is not a quitter' do
    index_membership('AAA', 'BBB')
    exited('CCC')
    @assets['CCC'][:ticker].update!(minimum_base_size: 1)
    stub_holdings('AAA' => 50, 'BBB' => 30, 'CCC' => 20)
    @bot.stubs(:metrics_with_current_prices).returns(
      asset_values: { 'CCC' => { amount: 0.004.to_d, current_value: 0.01.to_d } }, prices_stale: false
    )

    assert_empty @bot.exited_holdings, 'below the venue floor, so it can never be sold'
  end

  # Size alone is not the rule — size against the desired allocation is. A member is shown at any
  # amount, because the bot is going to keep buying it.
  test 'a member holding less than the venue floor is still a member' do
    index_membership('AAA')
    @assets['AAA'][:ticker].update!(minimum_base_size: 1)
    @bot.stubs(:metrics_with_current_prices).returns(
      asset_values: { 'AAA' => { amount: 0.004.to_d, current_value: 0.to_d } }, prices_stale: false
    )

    assert_empty @bot.exited_holdings, 'a member is never a quitter, whatever its size'
  end

  # A non-member above the floor is a quitter; one below it is invisible until there is more of it.
  test 'only a non-member below the floor is hidden' do
    index_membership('AAA')
    @assets['CCC'][:ticker].update!(minimum_base_size: 1)
    @bot.stubs(:metrics_with_current_prices).returns(
      asset_values: { 'AAA' => { amount: 5.to_d, current_value: 50.to_d },
                      'BBB' => { amount: 9.to_d, current_value: 90.to_d },
                      'CCC' => { amount: 0.004.to_d, current_value: 0.to_d } },
      prices_stale: false
    )

    assert_equal %w[BBB], @bot.exited_holdings.map { |h| h[:symbol] },
                 'AAA is a member, CCC is a non-member with too little to sell'
  end

  test 'a holding above the venue floor is still a quitter' do
    index_membership('AAA')
    exited('CCC')
    @assets['CCC'][:ticker].update!(minimum_base_size: 1)
    @bot.stubs(:metrics_with_current_prices).returns(
      asset_values: { 'CCC' => { amount: 5.to_d, current_value: 20.to_d } }, prices_stale: false
    )

    assert_equal(%w[CCC], @bot.exited_holdings.map { |h| h[:symbol] })
  end

  # The controller guards on exited_symbols; if the two disagreed a Sell would 404 from a row the
  # page still showed, or the job would refuse one the page offered.
  test 'the symbol list applies the same dust rule as the table' do
    index_membership('AAA')
    exited('CCC')
    @assets['CCC'][:ticker].update!(minimum_base_size: 1)
    @bot.stubs(:metrics).returns(asset_breakdown: { 'CCC' => { amount: 0.004.to_d, quote_invested: 1.to_d } })

    assert_empty @bot.exited_symbols
  end

  # == placing ==

  # == one holding at a time ==
  #
  # There is no "sell everything that left" any more. The button lives on the row, so the symbol is
  # what the user picked — and because it arrives in the URL it is untrusted input.

  test 'only the named quitter is sold' do
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })

    @bot.liquidate_exited!(symbol: 'CCC')

    assert_equal %w[CCC], @bot.transactions.liquidation.map(&:base)
  end

  test 'naming an index member sells nothing' do
    # Hand-editing the symbol in the URL must not reach a live constituent. The controller 404s on
    # it; this is the model refusing on its own so the job is safe whatever calls it.
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })

    result = @bot.liquidate_exited!(symbol: 'AAA')

    assert_predicate result, :failure?
    assert_empty @bot.transactions.liquidation
  end

  test 'naming a holding that does not exist sells nothing' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })

    result = @bot.liquidate_exited!(symbol: 'ZZZ')

    assert_predicate result, :failure?
    assert_empty @bot.transactions.liquidation
  end

  test 'the market-hours check asks only about the ticker being sold' do
    # liquidation_tickers feeds Exchange#market_open?. Asking with the whole catalogue refuses a
    # 24/7 crypto sale whenever the stock market happens to be shut.
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })

    assert_equal [@assets['CCC'][:ticker]], @bot.liquidation_tickers(symbol: 'CCC')
  end

  test 'the sell is capped at what is actually on the exchange' do
    # Coins moved to cold storage still count toward the portfolio but cannot be traded.
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 }, free: { 'CCC' => 0.15 })

    @bot.liquidate_exited!(symbol: 'CCC')

    assert_in_delta 0.15, @bot.transactions.liquidation.last.amount.to_f, 0.0001,
                    'held 0.2, but only 0.15 is on the exchange'
  end

  test 'liquidation orders are market orders and not contributions' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })

    @bot.liquidate_exited!(symbol: 'CCC')

    order = @bot.transactions.liquidation.last
    assert_equal 'market_order', order.order_type
    assert_equal 'LIQUIDATION', order.transaction_type
    assert_equal 'sell', order.side
  end

  # == per-holding skips ==

  test 'a quitter below the venue minimum is skipped' do
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })
    @assets['CCC'][:ticker].update!(minimum_quote_size: 1_000)

    @bot.liquidate_exited!(symbol: 'CCC')

    assert_empty @bot.transactions.liquidation
  end

  test 'a quitter with a resting DCA buy is skipped so the sale is not immediately undone' do
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :open,
                         external_id: 'open-buy', side: :buy, base: 'CCC', quote: @bot.quote_asset.symbol,
                         transaction_type: 'REGULAR', price: 100, amount: 1)
    @bot.stubs(:advance_waiting_orders!)

    @bot.liquidate_exited!(symbol: 'CCC')

    assert_empty @bot.transactions.liquidation
  end

  test 'a delisted quitter is skipped rather than raising' do
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })
    @assets['CCC'][:ticker].update!(available: false)

    @bot.liquidate_exited!(symbol: 'CCC')

    assert_empty @bot.transactions.liquidation
  end

  # == guards ==

  test 'refuses while a rebalance is mid-swap' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_SELLING)

    assert_predicate @bot.liquidate_exited!(symbol: 'CCC'), :failure?
    assert_empty @bot.transactions.liquidation
  end

  test 'refuses while one of its own orders is still working' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :open,
                         external_id: 'working', side: :sell, base: 'CCC', quote: @bot.quote_asset.symbol,
                         transaction_type: 'LIQUIDATION', price: 100, amount: 1)
    @bot.stubs(:advance_waiting_orders!)

    assert_predicate @bot.liquidate_exited!(symbol: 'CCC'), :failure?
    assert_equal 1, @bot.transactions.liquidation.count, 'no second order on top of the live one'
  end

  test 'refuses while halted' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    @bot.start_liquidation_placement!('CCC')
    @bot.flag_liquidation_ambiguous!

    assert_predicate @bot.liquidate_exited!(symbol: 'CCC'), :failure?
    assert_empty @bot.transactions.liquidation
  end

  test 'a failed composition refresh aborts rather than selling on a stale index' do
    # Unlike the rebalancer's best-effort refresh: here a stale composition would SELL an asset that
    # may have re-entered the index since the page was rendered.
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    @bot.stubs(:refresh_composition).returns(Result::Failure.new('upstream down'))

    assert_predicate @bot.liquidate_exited!(symbol: 'CCC'), :failure?
    assert_empty @bot.transactions.liquidation
  end

  # == unknown outcomes ==

  test 'an ambiguous placement halts' do
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })
    @bot.exchange.stubs(:market_sell).raises(Client::AmbiguousPlacementError, 'timeout')

    @bot.liquidate_exited!(symbol: 'CCC')

    assert_predicate @bot, :liquidation_ambiguous?
    assert_empty @bot.transactions.liquidation
  end

  test 'an accepted order with no usable id halts' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    @bot.exchange.stubs(:market_sell).returns(Result::Success.new(order_id: nil))

    @bot.liquidate_exited!(symbol: 'CCC')

    assert_predicate @bot, :liquidation_ambiguous?
  end

  test 'a failure that is not a proven pre-trade rejection halts' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    @bot.exchange.stubs(:market_sell).returns(Result::Failure.new('gateway timeout'))

    @bot.liquidate_exited!(symbol: 'CCC')

    assert_predicate @bot, :liquidation_ambiguous?
  end

  test 'a proven pre-trade rejection leaves no halt behind' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    @bot.exchange.stubs(:market_sell).returns(Result::Failure.new('Insufficient balance'))
    @bot.exchange.stubs(:placement_transient_error?).returns(true)

    @bot.liquidate_exited!(symbol: 'CCC')

    assert_not_predicate @bot, :liquidation_pending?
    assert_equal 'failed', @bot.transactions.liquidation.last.status
  end

  test 'a pre-transmission network error leaves no halt behind' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    @bot.exchange.stubs(:market_sell).raises(Client::TransientNetworkError, 'connection refused')

    @bot.liquidate_exited!(symbol: 'CCC')

    assert_not_predicate @bot, :liquidation_pending?
  end

  test 'an accepted order clears its own intent' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })

    @bot.liquidate_exited!(symbol: 'CCC')

    assert_not_predicate @bot, :liquidation_pending?
    assert_equal 1, @bot.transactions.liquidation.count
  end

  test 'intent that survived its worker is promoted to a visible halt, not a silent refusal' do
    # Holding the exchange semaphore is the proof that no placement of ours is still running.
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    @bot.start_liquidation_placement!('CCC')

    @bot.liquidate_exited!(symbol: 'CCC')

    assert_predicate @bot, :liquidation_ambiguous?
  end

  # == the other legs stand down ==

  test 'the DCA leg skips its tick while a liquidation is in flight' do
    index_membership('AAA')
    @bot.start_liquidation_placement!('CCC')

    assert_predicate @bot.execute_action, :success?
    assert_empty @bot.transactions
  end

  test 'a working liquidation order stands the DCA leg down even with no intent left' do
    index_membership('AAA')
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :open,
                         external_id: 'working', side: :sell, base: 'CCC', quote: @bot.quote_asset.symbol,
                         transaction_type: 'LIQUIDATION', price: 100, amount: 1)

    assert_predicate @bot, :liquidation_in_flight?
    assert_empty @bot.transactions.regular
  end

  test 'no new rebalance starts while a liquidation is in flight' do
    index_membership('AAA', 'BBB')
    @bot.settings = @bot.settings.merge('rebalance_enabled' => true, 'rebalance_threshold' => 0.05)
    @bot.set_missed_quote_amount
    @bot.save!
    @bot.start_liquidation_placement!('CCC')

    assert_not @bot.rebalance_due?
  end

  test 'the market-hours check asks about the assets being sold, not the whole catalogue' do
    # An index bot's `tickers` is every quote-matching ticker on the venue. Alpaca skips the stock
    # clock only when EVERY supplied ticker is crypto, so asking with the catalogue refuses a 24/7
    # crypto sale whenever the stock market happens to be shut.
    index_membership('AAA')
    exited('CCC')
    stub_holdings('AAA' => 50, 'CCC' => 20)

    assert_equal [@assets['CCC'][:ticker]], @bot.liquidation_tickers
  end

  test 'with nothing to sell the hours check falls back to the bot tickers' do
    index_membership('AAA')
    stub_holdings('AAA' => 50)

    assert_equal @bot.tickers.to_a, @bot.liquidation_tickers
  end

  test 'a DCA tick promotes a dead placement intent instead of skipping forever' do
    # The tick holds the same exchange semaphore a placement would, so surviving `placing` intent is
    # a dead worker. If the tick only ever returned, the bot would skip every contribution while the
    # halt it should be showing stayed invisible — and only a Sell click would ever surface it.
    index_membership('AAA')
    @bot.start_liquidation_placement!('CCC')

    @bot.execute_action

    assert_predicate @bot, :liquidation_ambiguous?
  end

  test 'the DCA tick sweeps the order it is standing down for, instead of waiting forever' do
    # This guard is prepended AHEAD of Bot::LimitOrderable, so returning early skips the open-order
    # sweep that would advance the very order being waited on. Without sweeping here the bot stands
    # down permanently on a row nothing else ever polls.
    index_membership('AAA')
    order = create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                                 external_status: :open, external_id: 'still-open', side: :sell,
                                 base: 'CCC', quote: @bot.quote_asset.symbol,
                                 transaction_type: 'LIQUIDATION', price: 100, amount: 1)
    @bot.stubs(:get_orders).returns(Result::Success.new(
                                      orders: { 'still-open' => { status: :closed, price: 100, amount: 1,
                                                                  quote_amount: 100, amount_exec: 1,
                                                                  quote_amount_exec: 100,
                                                                  ticker: @assets['CCC'][:ticker],
                                                                  side: :sell, order_type: :market_order } },
                                      missing: []
                                    ))

    @bot.execute_action

    assert_equal 'closed', order.reload.external_status
    assert_not_predicate @bot, :liquidation_in_flight?
  end

  test 'a resting DCA order alone does not trigger the liquidation sweep' do
    # Bot::LimitOrderable already sweeps for that one; firing here as well would double every
    # exchange read on any bot with a resting limit order.
    index_membership('AAA')
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :open,
                         external_id: 'dca-open', side: :buy, base: 'AAA', quote: @bot.quote_asset.symbol,
                         transaction_type: 'REGULAR', price: 100, amount: 1)
    @bot.expects(:get_orders).never

    assert_not @bot.liquidation_blocks_trading?
  end

  test 'an order the venue stopped reporting halts instead of freeing a retry' do
    # StaleOrderResolver abandons a >14d missing order, which takes it out of `waiting`. On a
    # non-authoritative venue that is not proof it never executed, and a fresh Sell placed on top of
    # an unrecorded fill can sell coins the bot does not own.
    index_membership('AAA')
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :open,
                         external_id: 'gone', side: :sell, base: 'CCC', quote: @bot.quote_asset.symbol,
                         transaction_type: 'LIQUIDATION', price: 100, amount: 1, created_at: 20.days.ago)
    @bot.stubs(:get_orders).returns(Result::Success.new(orders: {}, missing: %w[gone]))
    @bot.stubs(:broadcast_metrics_update)

    @bot.advance_waiting_orders!

    assert_predicate @bot, :liquidation_ambiguous?
    assert_equal 'CCC', @bot.liquidation_pending[:symbol]
  end

  test 'a halt repaints the widget, since nothing else will while trading is blocked' do
    index_membership('AAA')
    @bot.start_liquidation_placement!('CCC')
    @bot.expects(:broadcast_metrics_update).at_least_once

    @bot.promote_stale_liquidation_placement!
  end

  private

  def setup_liquidation(holdings, free: {})
    index_membership(holdings.keys.first)
    holdings.keys.drop(1).each { |symbol| exited(symbol) }
    stub_holdings(holdings)
    @bot.stubs(:metrics).returns(
      asset_breakdown: holdings.transform_values { |v| { amount: v.to_d / 100, quote_invested: v.to_d } }
    )
    @assets.each_value do |a|
      stub_ticker_bid_price(a[:ticker], price: 100)
      stub_ticker_ask_price(a[:ticker], price: 100)
    end
    # A distinct id per call: Mocha evaluates `returns` arguments ONCE, so a single generated id
    # would make the second order collide with the first in persist_accepted_order!'s idempotency
    # lookup — and the batch would silently place only one.
    @bot.exchange.stubs(:market_sell).returns(*(1..5).map { |i| Result::Success.new(order_id: "s-#{i}") })
    balances = @assets.to_h do |symbol, a|
      [a[:asset].id, { free: free[symbol] || 100.0, locked: 0 }]
    end
    balances[@bot.quote_asset_id] = { free: 10_000, locked: 0 }
    stub_exchange_balances(@bot.exchange, balances)
  end

  def index_membership(*symbols)
    symbols.each do |symbol|
      BotIndexAsset.create!(bot: @bot, asset: @assets[symbol][:asset], ticker: @assets[symbol][:ticker],
                            target_allocation: 1.0 / symbols.size, in_index: true, entered_at: Time.current)
    end
  end

  def exited(symbol)
    BotIndexAsset.create!(bot: @bot, asset: @assets[symbol][:asset], ticker: @assets[symbol][:ticker],
                          target_allocation: nil, in_index: false, exited_at: Time.current)
  end

  def stub_holdings(values)
    asset_values = values.transform_values do |v|
      { amount: v.to_d / 100, quote_invested: v.to_d, current_value: v.to_d, pnl_percentage: 0 }
    end
    @bot.stubs(:metrics_with_current_prices).returns(asset_values: asset_values, prices_stale: false)
  end
end
