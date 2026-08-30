require 'test_helper'

# The arithmetic behind "Redeploy $243?".
#
# The load-bearing claim: the offer is derived from quantities that only ever INCREASE — total
# liquidation proceeds and total redeploy spend — minus a snapshot of the same expression taken when
# the user last said No. Nothing here depends on the page having been rendered at the right moment,
# which is what two earlier designs got wrong: a cash watermark clamped on render, and a
# placement-time cutoff (`created_at` is when an order was placed, not when it filled).
class Bots::DcaIndexRedeployTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_index, user: create(:user))
    %w[AAA BBB].each do |symbol|
      asset = create(:asset, symbol: symbol, name: "Coin #{symbol}", external_id: "coin-#{symbol.downcase}")
      create(:ticker, exchange: @bot.exchange, base_asset: asset, quote_asset: @bot.quote_asset)
    end
  end

  test 'the whole of the realised cash is on offer when nothing has been declined' do
    buy('AAA', quote: 100, price: 100)
    liquidate('AAA', amount: 1, quote: 150, price: 150)

    assert_in_delta 150, @bot.redeploy_offer(@bot.metrics(force: true)).to_f, 0.0001
  end

  test 'declining takes the current proceeds off the table' do
    buy('AAA', quote: 100, price: 100)
    liquidate('AAA', amount: 1, quote: 150, price: 150)

    @bot.decline_redeploy!

    assert_in_delta 0, @bot.redeploy_offer(@bot.metrics(force: true)).to_f, 0.0001
  end

  # The case both earlier designs failed. Nothing reads the offer between the decline and the end,
  # so any scheme that corrects itself "on the next render" gets this wrong.
  test 'a later sale is offered on its own even with no read in between' do
    buy('AAA', quote: 300, price: 100)
    liquidate('AAA', amount: 2.43, quote: 243, price: 100)
    @bot.decline_redeploy!

    buy('BBB', quote: 100, price: 100) # the DCA leg drains 100 of the declined cash
    liquidate('AAA', amount: 0.5, quote: 50, price: 100) # a new quitter banks 50

    assert_in_delta 50, @bot.redeploy_offer(@bot.metrics(force: true)).to_f, 0.0001,
                    'only the new sale, never the 243 already declined'
  end

  test 'a redeploy that only partly fills leaves the remainder on offer' do
    buy('AAA', quote: 300, price: 100)
    liquidate('AAA', amount: 2.43, quote: 243, price: 100)
    @bot.decline_redeploy!
    liquidate('AAA', amount: 0.5, quote: 50, price: 100)

    redeploy('BBB', quote: 20, price: 100) # filled 20 of the 50, then cancelled

    assert_in_delta 30, @bot.redeploy_offer(@bot.metrics(force: true)).to_f, 0.0001
  end

  # A declined order that keeps filling is banking money AFTER the decline, so that part is new.
  test 'a declined order filling further offers only the part that landed after the decline' do
    buy('AAA', quote: 300, price: 100)
    order = liquidate('AAA', amount: 1, quote: 100, price: 100)
    @bot.decline_redeploy!

    order.update!(amount_exec: 1.3, quote_amount_exec: 130)

    assert_in_delta 30, @bot.redeploy_offer(@bot.metrics(force: true)).to_f, 0.0001
  end

  test 'the decline survives a cache flush' do
    buy('AAA', quote: 100, price: 100)
    liquidate('AAA', amount: 1, quote: 150, price: 150)
    @bot.decline_redeploy!

    Rails.cache.clear

    assert_in_delta 0, @bot.reload.redeploy_offer(@bot.metrics(force: true)).to_f, 0.0001
  end

  # rebalance_cash bundles the two; only one half is spendable. The other is owed to a half-finished
  # swap's own buy leg.
  test 'rebalance flight cash is never on offer' do
    buy('AAA', quote: 100, price: 100)
    rebalance_sell('AAA', amount: 0.5, quote: 50, price: 100)

    metrics = @bot.metrics(force: true)
    assert_in_delta 50, metrics[:rebalance_cash].to_f, 0.0001, 'it is in the bundled figure'
    assert_in_delta 0, @bot.redeploy_offer(metrics).to_f, 0.0001, 'but not on offer'
  end

  test 'a redeploy drains realised cash and nothing else' do
    buy('AAA', quote: 100, price: 100)
    rebalance_sell('AAA', amount: 0.5, quote: 10, price: 20) # 10 of flight cash
    liquidate('AAA', amount: 0.5, quote: 100, price: 200)

    redeploy('BBB', quote: 100, price: 100)

    metrics = @bot.metrics(force: true)
    assert_in_delta 0, metrics[:realised_cash].to_f, 0.0001, 'the whole 100 came from here'
    assert_in_delta 10, metrics[:rebalance_cash].to_f, 0.0001, 'the flight cash is untouched'
  end

  # If the redeploy had drained flight cash first (as a regular buy does), 10 of realised cash would
  # survive while the offer read zero — the sum of REDEPLOY fills is what the offer subtracts, so the
  # two have to measure the same money.
  test 'flight cash present does not strand realised cash out of the offer' do
    buy('AAA', quote: 100, price: 100)
    rebalance_sell('AAA', amount: 0.5, quote: 10, price: 20)
    liquidate('AAA', amount: 0.5, quote: 100, price: 200)
    redeploy('BBB', quote: 100, price: 100)

    liquidate('BBB', amount: 0.5, quote: 50, price: 100)

    assert_in_delta 50, @bot.redeploy_offer(@bot.metrics(force: true)).to_f, 0.0001
  end

  test 'recycling proceeds through a redeploy does not count them as new money' do
    buy('AAA', quote: 100, price: 100)
    liquidate('AAA', amount: 1, quote: 150, price: 150)
    before = @bot.metrics(force: true)

    redeploy('BBB', quote: 150, price: 100)

    after = @bot.metrics(force: true)
    assert_in_delta before[:total_quote_amount_invested].to_f,
                    after[:total_quote_amount_invested].to_f, 0.0001,
                    'the user contributed nothing new'
    assert_in_delta before[:realised_pnl].to_f, after[:realised_pnl].to_f, 0.0001,
                    'spending a realised gain does not un-realise it'
    assert_in_delta before[:total_amount_value_in_quote].to_f,
                    after[:total_amount_value_in_quote].to_f, 0.0001,
                    'cash became holdings; the total is unmoved'
  end

  test 'redeploy rows are not contributions, so the DCA carry is untouched' do
    @bot.update!(started_at: 2.days.ago, status: :waiting)
    before = @bot.pending_quote_amount

    liquidate('AAA', amount: 1, quote: 150, price: 150)
    redeploy('BBB', quote: 150, price: 100)

    assert_in_delta before.to_f, @bot.reload.pending_quote_amount.to_f, 0.0001
    assert_equal 0, @bot.transactions.regular.count
  end

  test 'declining is refused while a redeploy is still working' do
    buy('AAA', quote: 100, price: 100)
    liquidate('AAA', amount: 1, quote: 150, price: 150)
    create_order('BBB', amount: 1, quote: 150, price: 150, side: :buy, type: 'REDEPLOY',
                        external_status: :open)

    result = @bot.decline_redeploy!

    assert result.failure?, 'a decline taken mid-batch freezes the offset against a growing spend'
    assert_in_delta 0, @bot.reload.redeploy_declined_offset.to_d.to_f, 0.0001
  end

  # Unreachable once the guard above holds, so it is a bug state rather than a normal one — but a
  # stale offset left behind would silently eat every future sale, which is worse than re-anchoring.
  test 'an offset stranded above the banked total re-anchors instead of eating the next sale' do
    buy('AAA', quote: 300, price: 100)
    liquidate('AAA', amount: 1, quote: 100, price: 100)
    @bot.update_columns(redeploy_declined_offset: 500)

    assert_in_delta 0, @bot.redeploy_offer(@bot.metrics(force: true)).to_f, 0.0001
    assert_in_delta 100, @bot.reload.redeploy_declined_offset.to_d.to_f, 0.0001, 're-anchored'

    liquidate('AAA', amount: 0.5, quote: 50, price: 100)
    assert_in_delta 50, @bot.redeploy_offer(@bot.metrics(force: true)).to_f, 0.0001
  end

  test 'the offer never exceeds what the books still hold' do
    buy('AAA', quote: 100, price: 100)
    liquidate('AAA', amount: 1, quote: 150, price: 150)
    buy('BBB', quote: 120, price: 100) # the DCA leg has already spent most of it

    assert_in_delta 30, @bot.redeploy_offer(@bot.metrics(force: true)).to_f, 0.0001
  end

  # Bot::FetchAndUpdateOrderJob explicitly allows a closed sell whose base fill is known while its
  # quote fill is still nil; the ledger values those at price * amount. A plain SUM would read them
  # as zero, leaving realised_cash showing proceeds the offer could not see — permanently.
  test 'a fill valued from price and amount is still banked' do
    buy('AAA', quote: 100, price: 100)
    create_order('AAA', amount: 1, quote: 150, price: 150, side: :sell, type: 'LIQUIDATION')
    @bot.transactions.liquidation.last.update_columns(quote_amount_exec: nil)

    metrics = @bot.metrics(force: true)
    assert_in_delta 150, metrics[:realised_cash].to_f, 0.0001, 'the ledger values it'
    assert_in_delta 150, @bot.redeploy_offer(metrics).to_f, 0.0001, 'and so must the offer'
  end

  # Every other leg gates on the other two. This one was the only one nobody asked about.
  test 'a redeploy in flight stands the other legs down' do
    create_order('BBB', amount: 1, quote: 150, price: 150, side: :buy, type: 'REDEPLOY',
                        external_status: :open)
    @bot.stubs(:advance_waiting_redeploys!)

    assert @bot.redeploy_blocks_trading?, 'a waiting redeploy has spoken for that quote'
    assert_equal :redeploy_pending, @bot.send(:liquidation_blocked_reason)
    assert_not @bot.rebalance_due?
  end

  test 'a halted redeploy stands the other legs down too' do
    @bot.start_redeploy_placement!
    @bot.flag_redeploy_ambiguous!
    @bot.stubs(:advance_waiting_redeploys!)

    assert @bot.redeploy_blocks_trading?
    assert_equal :redeploy_pending, @bot.send(:liquidation_blocked_reason)
  end

  test 'a quiet leg blocks nothing' do
    @bot.stubs(:advance_waiting_redeploys!)

    assert_not @bot.redeploy_blocks_trading?
    assert_nil @bot.send(:liquidation_blocked_reason)
  end

  # A completed redeploy leaves banked and spent differing by rounding dust, and `positive?` is true
  # for that — so the prompt asked "Redeploy 0.00 USDT?" over two hundred-millionths of a cent.
  test 'accounting dust is not an offer' do
    buy('AAA', quote: 100, price: 100)
    liquidate('AAA', amount: 1, quote: 150, price: 150)
    redeploy('BBB', quote: 149.99999998, price: 100)

    # Exactly zero, not "near" it: the dust this guards against is 2e-8, far inside any delta a
    # money assertion would normally use — an assert_in_delta here could never fail.
    assert_predicate @bot.redeploy_offer(@bot.metrics(force: true)), :zero?
  end

  # The floor is the cheapest door in the composition, because the fold pools every share into one
  # member — so the pot only ever has to clear one of them.
  test 'an offer below the smallest venue minimum is not offered' do
    in_index('AAA', 'BBB')
    @bot.exchange.tickers.update_all(minimum_quote_size: 10)
    buy('AAA', quote: 100, price: 100)
    liquidate('AAA', amount: 0.05, quote: 6, price: 120)

    assert_in_delta 0, @bot.redeploy_offer(@bot.metrics(force: true)).to_f, 0.0001, 'under the floor'

    liquidate('AAA', amount: 0.05, quote: 6, price: 120)
    assert_in_delta 12, @bot.redeploy_offer(@bot.metrics(force: true)).to_f, 0.0001, 'over it'
  end

  private

  def in_index(*symbols)
    symbols.each do |symbol|
      ticker = @bot.exchange.tickers.find_by(base: symbol)
      BotIndexAsset.create!(bot: @bot, asset: ticker.base_asset, ticker: ticker,
                            target_allocation: 1.0 / symbols.size, in_index: true,
                            entered_at: Time.current)
    end
  end

  def buy(symbol, quote:, price:)
    create_order(symbol, amount: quote.to_d / price, quote:, price:, side: :buy, type: 'REGULAR')
  end

  def redeploy(symbol, quote:, price:)
    create_order(symbol, amount: quote.to_d / price, quote:, price:, side: :buy, type: 'REDEPLOY')
  end

  def rebalance_sell(symbol, amount:, quote:, price:)
    create_order(symbol, amount:, quote:, price:, side: :sell, type: 'REBALANCE')
  end

  def liquidate(symbol, amount:, quote:, price:)
    create_order(symbol, amount:, quote:, price:, side: :sell, type: 'LIQUIDATION')
  end

  def create_order(symbol, amount:, quote:, price:, side:, type:, external_status: :closed)
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                         external_status: external_status, external_id: "i-#{SecureRandom.hex(4)}",
                         side: side, transaction_type: type,
                         base: symbol, quote: @bot.quote_asset.symbol,
                         price: price, amount: amount,
                         amount_exec: external_status == :open ? 0 : amount,
                         quote_amount: quote,
                         quote_amount_exec: external_status == :open ? 0 : quote)
  end
end
