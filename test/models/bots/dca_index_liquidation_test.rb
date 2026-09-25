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
  # under "Out of the index" forever showing 0.00 over a Sell button that could never clear it.
  test 'a dust remainder is not a quitter' do
    index_membership('AAA', 'BBB')
    exited('CCC')
    @assets['CCC'][:ticker].update!(minimum_base_size: 1)
    stub_holdings('AAA' => 50, 'BBB' => 30, 'CCC' => 20)
    stubbed = keyed_payload(
      asset_values: { 'CCC' => { amount: 0.004.to_d, current_value: 0.01.to_d } }, prices_stale: false
    )
    @bot.stubs(:metrics_with_current_prices).returns(stubbed)

    assert_empty @bot.exited_holdings, 'below the venue floor, so it can never be sold'
  end

  # Size alone is not the rule — size against the desired allocation is. A member is shown at any
  # amount, because the bot is going to keep buying it.
  test 'a member holding less than the venue floor is still a member' do
    index_membership('AAA')
    @assets['AAA'][:ticker].update!(minimum_base_size: 1)
    stubbed = keyed_payload(
      asset_values: { 'AAA' => { amount: 0.004.to_d, current_value: 0.to_d } }, prices_stale: false
    )
    @bot.stubs(:metrics_with_current_prices).returns(stubbed)

    assert_empty @bot.exited_holdings, 'a member is never a quitter, whatever its size'
  end

  # A non-member above the floor is a quitter; one below it is invisible until there is more of it.
  test 'only a non-member below the floor is hidden' do
    index_membership('AAA')
    @assets['CCC'][:ticker].update!(minimum_base_size: 1)
    stubbed = keyed_payload(
      asset_values: { 'AAA' => { amount: 5.to_d, current_value: 50.to_d },
                      'BBB' => { amount: 9.to_d, current_value: 90.to_d },
                      'CCC' => { amount: 0.004.to_d, current_value: 0.to_d } },
      prices_stale: false
    )
    @bot.stubs(:metrics_with_current_prices).returns(stubbed)

    assert_equal %w[BBB], @bot.exited_holdings.map { |h| h[:symbol] },
                 'AAA is a member, CCC is a non-member with too little to sell'
  end

  test 'a holding above the venue floor is still a quitter' do
    index_membership('AAA')
    exited('CCC')
    @assets['CCC'][:ticker].update!(minimum_base_size: 1)
    stubbed = keyed_payload(
      asset_values: { 'CCC' => { amount: 5.to_d, current_value: 20.to_d } }, prices_stale: false
    )
    @bot.stubs(:metrics_with_current_prices).returns(stubbed)

    assert_equal(%w[CCC], @bot.exited_holdings.map { |h| h[:symbol] })
  end

  # The controller guards on exited_symbols; if the two disagreed a Sell would 404 from a row the
  # page still showed, or the job would refuse one the page offered.
  test 'the symbol list applies the same dust rule as the table' do
    index_membership('AAA')
    exited('CCC')
    @assets['CCC'][:ticker].update!(minimum_base_size: 1)
    @bot.stubs(:metrics).returns(keyed_payload(asset_breakdown: { 'CCC' => { amount: 0.004.to_d, quote_invested: 1.to_d } }))

    assert_empty @bot.exited_symbols
  end

  # == placing ==

  # == the positions the user named ==
  #
  # One row's Sell, or the band's Sell all carrying exactly the symbols that table rendered. Either
  # way the names arrive in the URL, so they are untrusted input and are refused here as well as in
  # the controller. Each is still a separate disposal: its own order, row, wash-sale verdict and
  # activity line, nothing netted.

  test 'only the named quitter is sold' do
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })

    @bot.liquidate!(holdings: holdings_named(%w[CCC]))

    assert_equal %w[CCC], @bot.transactions.liquidation.map(&:base)
  end

  test 'naming an index member sells that member' do
    # Direct indexing: any position can be closed on its own, a current constituent included.
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })

    result = @bot.liquidate!(holdings: holdings_named(%w[AAA]))

    assert_predicate result, :success?
    assert_equal %w[AAA], @bot.transactions.liquidation.map(&:base)
  end

  test 'naming a holding that does not exist sells nothing' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })

    result = @bot.liquidate!(holdings: holdings_named(%w[ZZZ]))

    assert_predicate result, :failure?
    assert_empty @bot.transactions.liquidation
  end

  # == the whole table at once ==

  test 'every named position gets its own order' do
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })

    result = @bot.liquidate!(holdings: holdings_named(%w[BBB CCC]))

    assert_predicate result, :success?
    assert_equal 2, result.data[:placed]
    assert_equal %w[BBB CCC], @bot.transactions.liquidation.map(&:base).sort
  end

  test 'a member the batch did not name is left alone' do
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })

    @bot.liquidate!(holdings: holdings_named(%w[BBB CCC]))

    assert_not_includes @bot.transactions.liquidation.map(&:base), 'AAA'
  end

  test 'the orders go in the order the confirmation listed' do
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })

    @bot.liquidate!(holdings: holdings_named(%w[CCC BBB]))

    assert_equal %w[CCC BBB], @bot.transactions.liquidation.order(:id).map(&:base)
  end

  test 'a repeated name places one order, not two' do
    # Both placements would be sized off the same snapshot, so a duplicate in the list is a double
    # sale. The callers dedupe; this is the money path's own guard.
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })

    @bot.liquidate!(holdings: holdings_named(%w[CCC CCC]))

    assert_equal 1, @bot.transactions.liquidation.count
  end

  test 'an unknown outcome stops the batch where it happened' do
    # The halt exists so nothing is sold twice. Continuing past it would place the very orders it is
    # meant to be blocking.
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })
    @bot.exchange.stubs(:market_sell)
        .returns(Result::Success.new(order_id: 's-1'))
        .then.raises(Client::AmbiguousPlacementError, 'timeout')

    @bot.liquidate!(holdings: holdings_named(%w[BBB CCC]))

    assert_equal %w[BBB], @bot.transactions.liquidation.map(&:base), 'only the one that landed'
    assert_predicate @bot, :liquidation_ambiguous?
    assert_equal 'CCC', @bot.liquidation_pending[:symbol], 'the halt names the one in doubt'
  end

  test 'a skipped position does not stop the ones after it' do
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })
    @assets['BBB'][:ticker].update!(minimum_quote_size: 1_000)

    result = @bot.liquidate!(holdings: holdings_named(%w[BBB CCC]))

    assert_equal %w[CCC], @bot.transactions.liquidation.map(&:base)
    assert_equal 1, result.data[:placed]
    assert_not_predicate @bot, :liquidation_ambiguous?
    skipped = @bot.bot_activity_logs.find_by(event: 'liquidation_skipped')
    assert_equal 'BBB', skipped.details['base']
  end

  test 'a name the bot no longer holds is dropped and the rest still sell' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })

    result = @bot.liquidate!(holdings: holdings_named(%w[CCC ZZZ]))

    assert_predicate result, :success?
    assert_equal %w[CCC], @bot.transactions.liquidation.map(&:base)
  end

  test 'a named position that is dropped says so instead of vanishing' do
    # For one symbol the :not_held refusal WAS the whole answer. In a batch the others still sell,
    # so a dropped one has to explain itself — otherwise the user asks for two, gets one, and
    # nothing anywhere says why. A live price missing for CCC is the realistic way in.
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })
    stubbed = keyed_payload(
      asset_values: { 'BBB' => { amount: 0.3.to_d, current_value: 30.to_d } }, prices_stale: false
    )
    @bot.stubs(:metrics_with_current_prices).returns(stubbed)

    result = @bot.liquidate!(holdings: holdings_named(%w[BBB CCC]))

    assert_predicate result, :success?
    assert_equal %w[BBB], @bot.transactions.liquidation.map(&:base)
    skipped = @bot.bot_activity_logs.where(event: 'liquidation_skipped')
    assert_equal 'CCC', skipped.sole.details['base']
    assert_equal 'not_held', skipped.sole.details['reason']
  end

  test 'each sale at a loss locks its own name and no other' do
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })
    @bot.user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
    @bot.stubs(:sell_at_loss?).returns(true)

    @bot.liquidate!(holdings: holdings_named(%w[BBB CCC]))

    locked = @bot.user.wash_sale_locks.live.pluck(:asset_id)
    assert_equal [@assets['BBB'][:asset].id, @assets['CCC'][:asset].id].sort, locked.sort
    assert_not_includes locked, @assets['AAA'][:asset].id, 'the member was never sold'
  end

  test 'the batch stops before its exclusion can lapse, and says what it did not try' do
    # The exchange semaphore is acquired at dispatch and never renewed. Overrun it and a second sale
    # can run beside us, with no intent and no waiting row between holdings to stand it down.
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })
    @bot.stubs(:liquidation_batch_deadline).returns(Time.current - 1.second)

    @bot.liquidate!(holdings: holdings_named(%w[BBB CCC]))

    assert_empty @bot.transactions.liquidation, 'the deadline was already past'
    cut = @bot.bot_activity_logs.find_by(event: 'liquidation_batch_cut_short')
    assert cut, 'a partial run has to say so where the user looks'
    assert_equal 'BBB, CCC', cut.details['bases']
  end

  test 'a lease that lapses during the price and balance reads stops the placement' do
    # The window is checked before the reads, but each of those is a network call that can take tens
    # of seconds — so it is asked again at the last moment nothing has been recorded or sent.
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    deadline = 30.seconds.from_now
    @bot.stubs(:side_price).with do
      travel 60.seconds
      true
    end.returns(100.to_d)

    result = @bot.liquidate!(holdings: holdings_named(%w[CCC]), deadline: deadline)

    assert_predicate result, :success?
    assert_empty @bot.transactions.liquidation, 'placing past the lease is what lets a second sale in'
    assert_equal 'exclusion_lapsed', @bot.bot_activity_logs.find_by(event: 'liquidation_skipped').details['reason']
  end

  test 'the market-hours check asks only about the ticker being sold' do
    # liquidation_tickers feeds Exchange#market_open?. Asking with the whole catalogue refuses a
    # 24/7 crypto sale whenever the stock market happens to be shut.
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })

    assert_equal [@assets['CCC'][:ticker]], @bot.liquidation_tickers(holdings: holdings_named(%w[CCC]))
  end

  test 'the market-hours check asks about every ticker in the batch' do
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })

    assert_equal [@assets['BBB'][:ticker], @assets['CCC'][:ticker]],
                 @bot.liquidation_tickers(holdings: holdings_named(%w[BBB CCC]))
  end

  test 'a name with no live price still reaches the market-hours check' do
    # Resolved off the ticker table, not off priced holdings. Through the holdings, a name whose
    # price read failed drops out — and a [crypto, stock] batch that loses its stock here is judged
    # all-crypto by Alpaca, skips the stock clock, then sells the stock anyway once
    # place_liquidation_orders! forces a fresh price.
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })
    stubbed = keyed_payload(
      asset_values: { 'BBB' => { amount: 0.3.to_d, current_value: 30.to_d } }, prices_stale: false
    )
    @bot.stubs(:metrics_with_current_prices).returns(stubbed)

    assert_equal [@assets['BBB'][:ticker], @assets['CCC'][:ticker]],
                 @bot.liquidation_tickers(holdings: holdings_named(%w[BBB CCC])),
                 'CCC has no price, but the clock still has to be asked about it'
  end

  test 'the sell is capped at what is actually on the exchange' do
    # Coins moved to cold storage still count toward the portfolio but cannot be traded.
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 }, free: { 'CCC' => 0.15 })

    @bot.liquidate!(holdings: holdings_named(%w[CCC]))

    assert_in_delta 0.15, @bot.transactions.liquidation.last.amount.to_f, 0.0001,
                    'held 0.2, but only 0.15 is on the exchange'
  end

  test 'liquidation orders are market orders and not contributions' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })

    @bot.liquidate!(holdings: holdings_named(%w[CCC]))

    order = @bot.transactions.liquidation.last
    assert_equal 'market_order', order.order_type
    assert_equal 'LIQUIDATION', order.transaction_type
    assert_equal 'sell', order.side
  end

  # == per-holding skips ==

  test 'a quitter below the venue minimum is skipped' do
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })
    @assets['CCC'][:ticker].update!(minimum_quote_size: 1_000)

    @bot.liquidate!(holdings: holdings_named(%w[CCC]))

    assert_empty @bot.transactions.liquidation
  end

  test 'a quitter with a resting DCA buy is skipped so the sale is not immediately undone' do
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :open,
                         external_id: 'open-buy', side: :buy, base: 'CCC', quote: @bot.quote_asset.symbol,
                         transaction_type: 'REGULAR', price: 100, amount: 1)
    @bot.stubs(:advance_waiting_orders!)

    @bot.liquidate!(holdings: holdings_named(%w[CCC]))

    assert_empty @bot.transactions.liquidation
  end

  test 'a delisted quitter is skipped rather than raising' do
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })
    @assets['CCC'][:ticker].update!(available: false)

    @bot.liquidate!(holdings: holdings_named(%w[CCC]))

    assert_empty @bot.transactions.liquidation
  end

  # == guards ==

  test 'refuses while a rebalance is mid-swap' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_SELLING)

    assert_predicate @bot.liquidate!(holdings: holdings_named(%w[CCC])), :failure?
    assert_empty @bot.transactions.liquidation
  end

  test 'refuses while one of its own orders is still working' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :open,
                         external_id: 'working', side: :sell, base: 'CCC', quote: @bot.quote_asset.symbol,
                         transaction_type: 'LIQUIDATION', price: 100, amount: 1)
    @bot.stubs(:advance_waiting_orders!)

    assert_predicate @bot.liquidate!(holdings: holdings_named(%w[CCC])), :failure?
    assert_equal 1, @bot.transactions.liquidation.count, 'no second order on top of the live one'
  end

  test 'refuses while one of its own scheduled sells is still working' do
    # A DCA-out sell resting on the book. Sell all on top of it would sell the same units twice over.
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :open,
                         external_id: 'scheduled', side: :sell, base: 'CCC', quote: @bot.quote_asset.symbol,
                         transaction_type: 'REGULAR', price: 100, amount: 0.1)
    @bot.stubs(:advance_waiting_orders!)

    assert_predicate @bot.liquidate!(holdings: holdings_named(%w[CCC])), :failure?
    assert_empty @bot.transactions.liquidation
  end

  test 'a scheduled sell last seen open is asked about before it blocks a stopped bot' do
    # Nothing polls a stopped bot's orders, so a sell that filled at the venue after the last look
    # still reads `open` here. Believed as-is it would refuse every Sell all for good.
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    scheduled = create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :open,
                                     external_id: 'scheduled', side: :sell, base: 'CCC', quote: @bot.quote_asset.symbol,
                                     transaction_type: 'REGULAR', price: 100, amount: 0.1)
    Bot::FetchAndUpdateOpenOrdersJob.expects(:perform_now).with do |bot, **|
      scheduled.update_columns(external_status: 'closed', amount_exec: 0.1, quote_amount_exec: 10) if bot == @bot
      true
    end

    assert_predicate @bot.liquidate!(holdings: holdings_named(%w[CCC])), :success?
    assert_equal 1, @bot.transactions.liquidation.count
  end

  test 'refuses while halted' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    @bot.start_liquidation_placement!('CCC')
    @bot.flag_liquidation_ambiguous!

    assert_predicate @bot.liquidate!(holdings: holdings_named(%w[CCC])), :failure?
    assert_empty @bot.transactions.liquidation
  end

  test 'a failed composition refresh does not stop a sale the user asked for' do
    # Best-effort, like the rebalancer's refresh. The strict refusal existed so a quitter that had
    # re-entered the index could not be sold; any position may now be sold on purpose.
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    @bot.stubs(:refresh_composition).returns(Result::Failure.new('upstream down'))

    assert_predicate @bot.liquidate!(holdings: holdings_named(%w[CCC])), :success?
    assert_equal %w[CCC], @bot.transactions.liquidation.map(&:base)
  end

  # == unknown outcomes ==

  test 'an ambiguous placement halts' do
    setup_liquidation({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })
    @bot.exchange.stubs(:market_sell).raises(Client::AmbiguousPlacementError, 'timeout')

    @bot.liquidate!(holdings: holdings_named(%w[CCC]))

    assert_predicate @bot, :liquidation_ambiguous?
    assert_empty @bot.transactions.liquidation
  end

  test 'an accepted order with no usable id halts' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    @bot.exchange.stubs(:market_sell).returns(Result::Success.new(order_id: nil))

    @bot.liquidate!(holdings: holdings_named(%w[CCC]))

    assert_predicate @bot, :liquidation_ambiguous?
  end

  test 'a failure that is not a proven pre-trade rejection halts' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    @bot.exchange.stubs(:market_sell).returns(Result::Failure.new('gateway timeout'))

    @bot.liquidate!(holdings: holdings_named(%w[CCC]))

    assert_predicate @bot, :liquidation_ambiguous?
  end

  test 'a proven pre-trade rejection leaves no halt behind' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    @bot.exchange.stubs(:market_sell).returns(Result::Failure.new('Insufficient balance'))
    @bot.exchange.stubs(:placement_transient_error?).returns(true)

    @bot.liquidate!(holdings: holdings_named(%w[CCC]))

    assert_not_predicate @bot, :liquidation_pending?
    assert_equal 'failed', @bot.transactions.liquidation.last.status
  end

  test 'a pre-transmission network error leaves no halt behind' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    @bot.exchange.stubs(:market_sell).raises(Client::TransientNetworkError, 'connection refused')

    @bot.liquidate!(holdings: holdings_named(%w[CCC]))

    assert_not_predicate @bot, :liquidation_pending?
  end

  test 'an accepted order clears its own intent' do
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })

    @bot.liquidate!(holdings: holdings_named(%w[CCC]))

    assert_not_predicate @bot, :liquidation_pending?
    assert_equal 1, @bot.transactions.liquidation.count
  end

  test 'intent that survived its worker is promoted to a visible halt, not a silent refusal' do
    # Holding the exchange semaphore is the proof that no placement of ours is still running.
    setup_liquidation({ 'AAA' => 50, 'CCC' => 20 })
    @bot.start_liquidation_placement!('CCC')

    @bot.liquidate!(holdings: holdings_named(%w[CCC]))

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

  test 'the market-hours check asks about the holdings, not the whole catalogue' do
    # An index bot's `tickers` is every quote-matching ticker on the venue. Alpaca skips the stock
    # clock only when EVERY supplied ticker is crypto, so asking with the catalogue refuses a 24/7
    # crypto sale whenever the stock market happens to be shut.
    index_membership('AAA')
    exited('CCC')
    stub_holdings('AAA' => 50, 'CCC' => 20)

    assert_equal [@assets['AAA'][:ticker], @assets['CCC'][:ticker]], @bot.liquidation_tickers
  end

  test 'with nothing to sell the hours check falls back to the bot tickers' do
    index_membership('AAA')
    stub_holdings({})

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
    abandon('CCC')

    @bot.advance_waiting_orders!

    assert_predicate @bot, :liquidation_halted?
    assert_equal %w[CCC], @bot.halted_liquidation_bases
    assert_predicate @bot.liquidate!(holdings: holdings_named(%w[CCC])), :failure?
  end

  test 'every order the venue gave up on blocks, not just the first' do
    # A sale can cover several positions, so several can be outstanding at once. One naming the
    # first would let its attestation lift the block while the others are unaccounted for.
    index_membership('AAA')
    abandon('BBB', 'CCC')

    @bot.advance_waiting_orders!

    assert_equal %w[BBB CCC], @bot.halted_liquidation_bases.sort
  end

  test 'an order given up on behind a standing halt is not lost' do
    # A batch can accept BBB and then halt on CCC. When BBB is later abandoned it leaves `waiting`
    # too and no sweep looks at it again, so the ROW has to carry its own block.
    index_membership('AAA')
    abandon('BBB')
    @bot.start_liquidation_placement!('CCC')
    @bot.flag_liquidation_ambiguous!

    @bot.advance_waiting_orders!

    assert_equal %w[CCC BBB], @bot.halted_liquidation_bases
  end

  test 'clearing the intent leaves an unaccounted-for order still blocking' do
    # The heart of it: one attestation must not lift the block for a sale it never covered.
    index_membership('AAA')
    abandon('BBB')
    @bot.start_liquidation_placement!('CCC')
    @bot.flag_liquidation_ambiguous!
    @bot.advance_waiting_orders!

    @bot.clear_liquidation_pending!

    assert_predicate @bot, :liquidation_halted?
    assert_equal %w[BBB], @bot.halted_liquidation_bases
    assert_predicate @bot.liquidate!(holdings: holdings_named(%w[BBB])), :failure?
  end

  test 'attesting about the listed orders clears them, and only them' do
    index_membership('AAA')
    listed = abandon('BBB')
    @bot.advance_waiting_orders!
    later = abandon('CCC')
    @bot.advance_waiting_orders!

    @bot.resolve_liquidation_orders!(listed.map(&:id))

    assert_equal %w[CCC], @bot.halted_liquidation_bases,
                 'an order given up on after the render was never part of the answer'
    @bot.resolve_liquidation_orders!(later.map(&:id))
    assert_not_predicate @bot, :liquidation_halted?
  end

  test 'a placement still in flight is never overwritten by the abandoned sweep' do
    # `placing` means a network call may be happening right now; its outcome is not the sweep's to
    # decide. The abandoned row blocks on its own account either way.
    index_membership('AAA')
    abandon('BBB')
    @bot.start_liquidation_placement!('CCC')

    @bot.advance_waiting_orders!

    assert_equal 'CCC', @bot.liquidation_pending[:symbol]
    assert_not_predicate @bot, :liquidation_ambiguous?
    assert_predicate @bot, :liquidation_halted?
  end

  test 'a halt repaints the widget, since nothing else will while trading is blocked' do
    index_membership('AAA')
    @bot.start_liquidation_placement!('CCC')
    @bot.expects(:broadcast_metrics_update).at_least_once

    @bot.promote_stale_liquidation_placement!
  end

  test 'a Sell after an unpriced partial sale submits what the bot still holds, not what it once held' do
    index_membership('AAA')
    @bot.stubs(:live_free_balance).returns(2.to_d) # the account holds two: one is somebody else's
    @bot.stubs(:side_price).returns(100.to_d)
    fresh = keyed_payload(asset_breakdown: { 'AAA' => { amount: 1.to_d, quote_invested: 100.to_d } })

    order = @bot.send(:liquidation_order_data, { symbol: 'AAA', ticker: @assets['AAA'][:ticker] }, fresh)

    assert_equal 1.to_d, order[:amount]
  end

  # == the wash-sale clock ==

  def wash_lock = @bot.user.wash_sale_locks.find_by(asset: @assets['AAA'][:asset])

  def holding
    { symbol: 'AAA', ticker: @assets['AAA'][:ticker], amount: 2, quote_invested: 200, tax_basis: 200, current_value: 180 }
  end

  def choose_us
    @bot.user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
  end

  # The lots say the 2 units cost 100 each. `amount` is what the sale submits: the exchange may hold
  # less than the position (cold storage), and the verdict is about the units actually sold.
  def placement_stubs(price:, amount: 2)
    @bot.stubs(:liquidation_order_data).returns(ticker: @assets['AAA'][:ticker], price: price, amount: amount.to_d,
                                                quote_amount: (amount * price).to_d, side: :sell, order_type: :market_order,
                                                transaction_type: 'LIQUIDATION')
    @bot.stubs(:calculate_best_amount_info).returns(below_minimum_amount: false)
    @bot.stubs(:metrics).returns(keyed_payload(asset_breakdown: {}, asset_lots: { 'AAA' => [{ amount: 2.to_d, cost: 200.to_d }] }))
  end

  def placed_stubs
    @bot.stubs(:create_order).returns(Result::Success.new(order_id: 'x'))
    @bot.stubs(:persist_accepted_order!).returns(@bot.transactions.build)
    Bot::FetchAndUpdateOrderJob.stubs(:perform_later)
  end

  test 'the lock is written with the placement intent, before the network call' do
    index_membership('AAA')
    choose_us
    placement_stubs(price: 90)
    seen_locked = nil
    watcher = lambda do |*_args|
      seen_locked = wash_lock&.reload&.buy_locked?
      true
    end
    @bot.stubs(:create_order).with(&watcher).returns(Result::Success.new(order_id: 'x'))
    @bot.stubs(:persist_accepted_order!).returns(@bot.transactions.build)
    Bot::FetchAndUpdateOrderJob.stubs(:perform_later)

    assert_equal :placed, @bot.send(:liquidate_holding!, holding, {})
    assert seen_locked, 'locked when the order went out'
    assert_predicate wash_lock.reload, :buy_locked?
    assert_equal 'AAA', @bot.bot_activity_logs.find_by(event: 'wash_sale_locked').details['base']
  end

  test 'the verdict is about the units submitted, not the whole holding' do
    index_membership('AAA')
    choose_us
    # Holding of 2 is under water as a whole, but the one unit that can be sold (FIFO cost 100)
    # goes for 105: a gain. No lock.
    placement_stubs(price: 105, amount: 1)
    placed_stubs

    @bot.send(:liquidate_holding!, holding, {})

    assert_not wash_lock&.reload&.buy_locked?
  end

  test 'a provable pre-transmission failure restores the previous deadline, not nothing' do
    index_membership('AAA')
    choose_us
    earlier = 5.days.from_now.beginning_of_day
    WashSaleLock.create!(user: @bot.user, asset: @assets['AAA'][:asset], buy_locked_until: earlier) # an older sale's lock
    placement_stubs(price: 90)
    @bot.stubs(:create_order).raises(Client::TransientNetworkError.new('dns'))

    @bot.send(:liquidate_holding!, holding, {})

    assert_equal earlier, wash_lock.reload.buy_locked_until, 'the older lock survives the failed second sale'
  end

  test 'a provable pre-transmission failure with no earlier lock leaves none' do
    index_membership('AAA')
    choose_us
    placement_stubs(price: 90)
    @bot.stubs(:create_order).raises(Client::TransientNetworkError.new('dns'))

    @bot.send(:liquidate_holding!, holding, {})

    assert_nil wash_lock&.reload&.buy_locked_until
  end

  test 'an ambiguous outcome keeps the lock' do
    index_membership('AAA')
    choose_us
    placement_stubs(price: 90)
    @bot.stubs(:create_order).raises(Client::AmbiguousPlacementError.new('timeout'))

    assert_equal :ambiguous, @bot.send(:liquidate_holding!, holding, {})
    assert_predicate wash_lock.reload, :buy_locked?
  end

  test 'a gain sale sets no lock' do
    index_membership('AAA')
    choose_us
    placement_stubs(price: 110)
    placed_stubs

    @bot.send(:liquidate_holding!, holding.merge(current_value: 220), {})

    assert_not wash_lock&.reload&.buy_locked?
  end

  test 'a sale of units whose cost we never learned is locked provisionally, kept on ambiguity, restored on a pre-transmission failure' do
    index_membership('AAA')
    choose_us
    placement_stubs(price: 110)
    @bot.stubs(:metrics).returns(keyed_payload(asset_breakdown: {}, asset_lots: { 'AAA' => [{ amount: 2.to_d, cost: nil }] }))

    @bot.stubs(:create_order).raises(Client::AmbiguousPlacementError.new('timeout'))
    assert_equal :ambiguous, @bot.send(:liquidate_holding!, holding, {})
    assert_predicate wash_lock.reload, :buy_locked?, 'unknown reads as a loss, and an ambiguous sale keeps it'

    wash_lock.update!(buy_locked_until: nil)
    @bot.stubs(:create_order).raises(Client::TransientNetworkError.new('dns'))
    @bot.send(:liquidate_holding!, holding, {})
    assert_nil wash_lock.reload.buy_locked_until, 'never left: restored to what was there, which was nothing'
  end

  private

  def setup_liquidation(holdings, free: {})
    index_membership(holdings.keys.first)
    holdings.keys.drop(1).each { |symbol| exited(symbol) }
    stub_holdings(holdings)
    stubbed = keyed_payload(
      asset_breakdown: holdings.transform_values { |v| { amount: v.to_d / 100, quote_invested: v.to_d } }
    )
    @bot.stubs(:metrics).returns(stubbed)
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

  # Orders the venue has stopped reporting: placed, then old enough for StaleOrderResolver.
  def abandon(*bases)
    @bot.stubs(:broadcast_metrics_update)
    rows = bases.map do |base|
      create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :open,
                           external_id: "gone-#{base}", side: :sell, base: base, quote: @bot.quote_asset.symbol,
                           transaction_type: 'LIQUIDATION', price: 100, amount: 1, created_at: 20.days.ago)
    end
    @bot.stubs(:get_orders).returns(Result::Success.new(orders: {}, missing: rows.map(&:external_id)))
    rows
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
    @bot.stubs(:metrics_with_current_prices).returns(keyed_payload(asset_values: asset_values, prices_stale: false))
    # held_symbols reads the price-free ledger, so the same holdings have to be there too.
    @bot.stubs(:metrics).returns(keyed_payload(asset_breakdown: asset_values.transform_values { |v| { amount: v[:amount] } }))
  end

  # == any position is sellable ==

  test 'members and quitters alike are sellable holdings' do
    index_membership('AAA', 'BBB')
    exited('CCC')
    stub_holdings('AAA' => 50, 'BBB' => 30, 'CCC' => 20)

    assert_equal %w[AAA BBB CCC], @bot.sellable_holdings.map { |h| h[:symbol] }.sort
    assert_equal %w[AAA BBB CCC], @bot.held_symbols.sort
  end

  test 'dust is not sellable, member or not' do
    index_membership('AAA', 'BBB')
    @assets['BBB'][:ticker].update!(minimum_base_size: 1)
    stub_holdings('AAA' => 50, 'BBB' => 30) # BBB amount 0.3 < 1

    assert_equal(%w[AAA], @bot.sellable_holdings.map { |h| h[:symbol] })
    assert_equal %w[AAA], @bot.held_symbols
  end

  test 'selling a current member is no longer refused' do
    index_membership('AAA', 'BBB')
    stub_holdings('AAA' => 50, 'BBB' => 30)
    @bot.expects(:liquidate_holding!).with { |holding, _| holding[:symbol] == 'AAA' }.returns(:placed)

    assert_predicate @bot.liquidate!(holdings: holdings_named(%w[AAA])), :success?
  end

  test 'a symbol the bot does not hold is refused' do
    index_membership('AAA')
    stub_holdings('AAA' => 50)

    result = @bot.liquidate!(holdings: holdings_named(%w[ZZZ]))

    assert_predicate result, :failure?
    assert_equal [:not_held], result.errors
  end
end
