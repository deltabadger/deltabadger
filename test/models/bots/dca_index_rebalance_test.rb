require 'test_helper'

# The index bot's share of the rebalance machinery is one method — rebalance_targets — and
# everything else (drift, the state machine, placement) is the concern the dual-asset bot already
# exercises. So these test the target list and what falls out of it across N assets, not the machine.
class Bots::DcaIndexRebalanceTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_index, user: create(:user), with_api_key: true)
    @assets = %w[AAA BBB CCC].map.with_index do |symbol, i|
      asset = create(:asset, symbol: symbol, name: "Coin #{symbol}", external_id: "coin-#{symbol.downcase}")
      ticker = create(:ticker, exchange: @bot.exchange, base_asset: asset, quote_asset: @bot.quote_asset)
      [symbol, { asset: asset, ticker: ticker, rank: i }]
    end.to_h
    @bot.instance_variable_set(:@tickers, @assets.values.map { |a| a[:ticker] })
  end

  test 'in-index assets carry their index weight' do
    index_membership('AAA' => 0.5, 'BBB' => 0.3, 'CCC' => 0.2)
    stub_values({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })

    targets = @bot.send(:rebalance_targets).index_by { |t| t[:ticker].base }

    assert_in_delta 0.5, targets['AAA'][:target].to_f, 0.0001
    assert_in_delta 0.2, targets['CCC'][:target].to_f, 0.0001
  end

  test 'an asset that has left the index is not a rebalance target at all' do
    # Rebalancing tracks the index, and a quitter is not in it. Closing that position is the user's
    # call (Bot::Composition::Liquidatable), not a side effect of some other asset breaching the band.
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    exited('CCC')
    stub_values({ 'AAA' => 40, 'BBB' => 40, 'CCC' => 20 })

    bases = @bot.send(:rebalance_targets).map { |t| t[:ticker].base }

    assert_equal %w[AAA BBB], bases.sort
  end

  test 'an exited holding is out of the denominator, not just the candidates' do
    # The weights describe the INDEX, so a holding outside it must not dilute them. With CCC in the
    # total, AAA at 40 of 100 would read 10 points light against its 50 % target and trade for
    # nothing.
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    exited('CCC')
    stub_values({ 'AAA' => 40, 'BBB' => 40, 'CCC' => 20 })

    total = @bot.send(:rebalance_targets).sum { |t| t[:value] }

    assert_in_delta 80, total.to_f, 0.0001
    assert_in_delta 0, @bot.rebalance_drift.to_f, 0.0001, 'the index half is perfectly balanced'
  end

  test 'drift is the largest single-asset deviation, not an average' do
    index_membership('AAA' => 0.34, 'BBB' => 0.33, 'CCC' => 0.33)
    stub_values({ 'AAA' => 60, 'BBB' => 20, 'CCC' => 20 })

    # AAA is 60 % against a 34 % target: 26 points. The others are 13 points under.
    assert_in_delta 0.26, @bot.rebalance_drift.to_f, 0.001
  end

  test 'an exited holding does not drive drift' do
    # It used to carry target 0, which made its whole value read as drift — so it was liquidated the
    # moment anything tripped the band, churning a coin that merely hovers at the index boundary.
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    exited('CCC')
    stub_values({ 'AAA' => 45, 'BBB' => 45, 'CCC' => 10 })

    assert_in_delta 0, @bot.rebalance_drift.to_f, 0.001
  end

  test 'the most overweight asset is the one sold' do
    index_membership('AAA' => 0.34, 'BBB' => 0.33, 'CCC' => 0.33)
    stub_values({ 'AAA' => 60, 'BBB' => 30, 'CCC' => 10 })
    enable_rebalancing

    @bot.rebalance!

    order = @bot.transactions.last
    assert_equal 'sell', order.side
    assert_equal 'AAA', order.base
  end

  test 'an exited asset is never the one sold, however large it has grown' do
    # Under the old target-0 rule its whole holding was excess, so it outranked every genuine
    # correction and was liquidated without the user ever asking.
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    exited('CCC')
    stub_values({ 'AAA' => 70, 'BBB' => 30, 'CCC' => 40 })
    enable_rebalancing

    @bot.rebalance!

    assert_equal 'AAA', @bot.transactions.last.base, 'the overweight INDEX asset, not the quitter'
  end

  test 'the proceeds go to the most underweight in-index asset' do
    index_membership('AAA' => 0.34, 'BBB' => 0.33, 'CCC' => 0.33)
    stub_values({ 'AAA' => 60, 'BBB' => 30, 'CCC' => 10 })
    enable_rebalancing
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING, remaining_quote_amount: 26)

    @bot.rebalance!

    assert_equal 'CCC', @bot.transactions.where(side: :buy).last.base
  end

  test 'an exited asset is never bought back into' do
    # It is not in the target list, so it can be neither the most underweight nor a buy candidate.
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    exited('CCC')
    stub_values({ 'AAA' => 50, 'BBB' => 10, 'CCC' => 40 })
    enable_rebalancing
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING, remaining_quote_amount: 30)

    @bot.rebalance!

    assert_equal 'BBB', @bot.transactions.where(side: :buy).last.base
  end

  test 'an asset the index wants but the bot does not hold yet is buyable' do
    index_membership('AAA' => 0.5, 'BBB' => 0.25, 'CCC' => 0.25)
    stub_values({ 'AAA' => 70, 'BBB' => 30 }) # nothing in CCC at all
    enable_rebalancing
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING, remaining_quote_amount: 25)

    @bot.rebalance!

    assert_equal 'CCC', @bot.transactions.where(side: :buy).last.base
  end

  test 'a delisted asset drops out of the candidates instead of blocking the rebalance' do
    index_membership('AAA' => 0.5, 'BBB' => 0.5, 'CCC' => 0.0)
    @assets['CCC'][:ticker].update!(available: false)
    stub_values({ 'AAA' => 70, 'BBB' => 10, 'CCC' => 20 })
    enable_rebalancing

    @bot.rebalance!

    assert_equal 'AAA', @bot.transactions.last.base, 'the rest of the portfolio still rebalances'
  end

  test 'an in-index asset with no weight is treated as on target, not as a liquidation' do
    # A missing number must never be read as "sell it all".
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    BotIndexAsset.create!(bot: @bot, asset: @assets['CCC'][:asset], ticker: @assets['CCC'][:ticker],
                          target_allocation: nil, in_index: true, entered_at: Time.current)
    stub_values({ 'AAA' => 40, 'BBB' => 40, 'CCC' => 20 })

    targets = @bot.send(:rebalance_targets).index_by { |t| t[:ticker].base }

    assert_in_delta 0.2, targets['CCC'][:target].to_f, 0.0001, 'its own share, so its deviation is zero'
  end

  # == The findings from reviewing this branch ==

  test 'the buy is capped at what the target is actually short, not the whole proceeds' do
    # On two assets the excess freed always exactly equals the other side's shortfall. On three it
    # does not, and spending it all into one asset overshoots — the next poll then sells back what
    # this one bought, for two fees and a taxable disposal and no change in allocation.
    # The post-sell state: 26 was raised from AAA and is sitting as cash awaiting deployment.
    index_membership('AAA' => 0.34, 'BBB' => 0.33, 'CCC' => 0.33)
    stub_values({ 'AAA' => 34, 'BBB' => 20, 'CCC' => 20 })
    enable_rebalancing
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING, remaining_quote_amount: 26)

    @bot.rebalance!

    # BBB is short 0.33 * (74 + 26) - 20 = 13, not the full 26 on offer.
    assert_in_delta 13, @bot.transactions.where(side: :buy).last.quote_amount.to_f, 0.5
  end

  test 'nothing is bought when the only tradeable candidate is already over its target' do
    # The genuinely underweight asset can become untradeable between the two legs. Picking the
    # least-overweight survivor would put the proceeds straight back into something already over
    # its weight — usually the very asset just sold.
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    stub_values({ 'AAA' => 60, 'BBB' => 40 })
    enable_rebalancing
    @assets['BBB'][:ticker].update!(available: false) # the underweight one drops out
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING, remaining_quote_amount: 20)

    @bot.exchange.expects(:market_buy).never
    @bot.rebalance!

    assert_equal Bot::Rebalanceable::PHASE_BUYING, @bot.reload.rebalance_pending[:phase],
                 'the cash stays owed rather than being spent badly'
  end

  test 'a held asset the price feed skipped defers the rebalance instead of valuing it at zero' do
    # A bulk price response can succeed but omit a symbol, and metrics does not flag that as stale.
    # Reading the holding as worthless would manufacture drift, sell other assets to fund it, and
    # buy more of the "worthless" one.
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    @bot.stubs(:metrics_with_current_prices).returns(
      asset_values: { 'AAA' => { amount: 1, current_value: 50.to_d } },
      asset_breakdown: { 'AAA' => { amount: 1 }, 'BBB' => { amount: 1 } },
      prices_stale: false
    )

    assert_nil @bot.send(:rebalance_targets)
    assert_nil @bot.rebalance_drift
  end

  test 'a delisted holding does not wedge rebalancing forever' do
    # It can never be priced again, so waiting for its price would stop the bot permanently.
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    @assets['CCC'][:ticker].update!(available: false)
    @bot.instance_variable_set(:@tickers, @assets.values.reject { |a| a[:ticker] == @assets['CCC'][:ticker] }.map { |a| a[:ticker] })
    @bot.stubs(:metrics_with_current_prices).returns(
      asset_values: { 'AAA' => { amount: 1, current_value: 60.to_d }, 'BBB' => { amount: 1, current_value: 40.to_d } },
      asset_breakdown: { 'AAA' => { amount: 1 }, 'BBB' => { amount: 1 }, 'CCC' => { amount: 1 } },
      prices_stale: false
    )

    assert_not_nil @bot.rebalance_drift
  end

  test 'a stopped bot refreshes the index composition before rebalancing' do
    # Its only other refresh is the DCA tick, which a stopped bot never runs — so without this it
    # would rebalance forever toward the composition frozen when it stopped.
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    stub_values({ 'AAA' => 50, 'BBB' => 50 })
    enable_rebalancing
    @bot.update_columns(status: Bot.statuses[:stopped])

    @bot.expects(:refresh_composition).returns(Result::Success.new)
    @bot.rebalance!
  end

  test 'stale prices stop the index bot too' do
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    stub_values({ 'AAA' => 90, 'BBB' => 10 }, stale: true)
    enable_rebalancing

    assert_nil @bot.rebalance_drift
    assert_not @bot.rebalance_due?
  end

  test 'a locked constituent is neither a rebalance candidate nor in the denominator' do
    index_membership('AAA' => 0.5, 'BBB' => 0.3, 'CCC' => 0.2)
    @bot.bot_index_assets.find_by(asset: @assets['CCC'][:asset]).update!(buy_locked_until: 10.days.from_now)
    stub_values({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 0 })

    targets = @bot.send(:rebalance_targets).index_by { |t| t[:ticker].base }

    assert_nil targets['CCC']
    assert_in_delta 0.625, targets['AAA'][:target].to_f, 0.0001
    assert_in_delta 0.375, targets['BBB'][:target].to_f, 0.0001
    assert_in_delta 0, @bot.rebalance_drift.to_f, 0.0001, 'the unlocked part is exactly on target'
  end

  test 'an unknown-weight survivor keeps its current share and the known weights fill the rest' do
    index_membership('AAA' => 0.5, 'BBB' => 0.3)
    @bot.bot_index_assets.create!(asset: @assets['CCC'][:asset], ticker: @assets['CCC'][:ticker], target_allocation: nil, in_index: true,
                                  entered_at: Time.current)
    @bot.bot_index_assets.find_by(asset: @assets['BBB'][:asset]).update!(buy_locked_until: 10.days.from_now)
    stub_values({ 'AAA' => 70, 'BBB' => 0, 'CCC' => 30 })

    targets = @bot.send(:rebalance_targets).index_by { |t| t[:ticker].base }

    assert_in_delta 0.3, targets['CCC'][:target].to_f, 0.0001, 'its own current share among survivors, so it reads on target'
    assert_in_delta 0.7, targets['AAA'][:target].to_f, 0.0001, 'the one known survivor takes all the remaining mass'
    assert_in_delta 1.0, targets.values.sum { |t| t[:target].to_f }, 0.0001
    assert_in_delta 0, @bot.rebalance_drift.to_f, 0.0001
  end

  test 'targets always sum to one, locks or not' do
    index_membership('AAA' => 0.6, 'BBB' => 0.4)
    @bot.bot_index_assets.create!(asset: @assets['CCC'][:asset], ticker: @assets['CCC'][:ticker], target_allocation: nil, in_index: true,
                                  entered_at: Time.current)
    stub_values({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })

    targets = @bot.send(:rebalance_targets)

    assert_in_delta 1.0, targets.sum { |t| t[:target].to_f }, 0.0001
    assert_in_delta 0.2, targets.find { |t| t[:ticker].base == 'CCC' }[:target].to_f, 0.0001
    assert_in_delta 0.48, targets.find { |t| t[:ticker].base == 'AAA' }[:target].to_f, 0.0001 # 0.6 x 0.8
  end

  test 'the buy leg still finds a name when every unlocked holding is worth nothing' do
    # 50/50, AAA worth 100 and just sold at a loss (locked), BBB never bought. The proceeds must go
    # somewhere, or the rebalance sits pending and the DCA leg stands down behind it for a month.
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    @bot.bot_index_assets.find_by(asset: @assets['AAA'][:asset]).update!(buy_locked_until: 10.days.from_now)
    stub_values({ 'AAA' => 50, 'BBB' => 0 })
    @bot.stubs(:side_price).returns(10.to_d)
    @bot.stubs(:live_free_balance).returns(50.to_d)

    order = @bot.send(:rebalance_buy_order_data, quote_amount: 50.to_d)

    assert_equal 'BBB', order[:ticker].base
    assert_in_delta 50, order[:quote_amount].to_f, 0.0001
  end

  test 'when unknown-weight holdings carry all the value, the recorded weights scale to zero' do
    index_membership('AAA' => 0.6, 'BBB' => 0.4)
    @bot.bot_index_assets.create!(asset: @assets['CCC'][:asset], ticker: @assets['CCC'][:ticker], target_allocation: nil, in_index: true,
                                  entered_at: Time.current)
    stub_values({ 'AAA' => 0, 'BBB' => 0, 'CCC' => 100 })

    targets = @bot.send(:rebalance_targets).index_by { |t| t[:ticker].base }

    assert_in_delta 1.0, targets['CCC'][:target].to_f, 0.0001
    assert_in_delta 0, targets['AAA'][:target].to_f, 0.0001
    assert_in_delta 1.0, targets.values.sum { |t| t[:target].to_f }, 0.0001
    assert_in_delta 0, @bot.rebalance_drift.to_f, 0.0001, 'no drift manufactured out of a missing number'
  end

  # == the wash-sale clock ==

  test 'a rebalance sell at a loss on the FIFO lots locks the name, a pre-transmission failure restores what was there' do
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    @bot.set_missed_quote_amount
    @bot.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
    @bot.stubs(:metrics).returns(asset_breakdown: {}, asset_lots: { 'AAA' => [{ amount: 2.to_d, cost: 200.to_d }] })
    @bot.stubs(:rebalance_sell_order_data).returns(ticker: @assets['AAA'][:ticker], price: 80, amount: 1.to_d,
                                                   quote_amount: 80.to_d, side: :sell, order_type: :market_order,
                                                   transaction_type: 'REBALANCE')
    @bot.stubs(:calculate_best_amount_info).returns(below_minimum_amount: false)
    @bot.stubs(:create_order).raises(Client::TransientNetworkError.new('dns'))

    @bot.send(:start_rebalance!)

    assert_nil @bot.bot_index_assets.find_by(asset: @assets['AAA'][:asset]).buy_locked_until
    assert_not_predicate @bot, :rebalance_pending?
    assert_nil @bot.reload.transient_data['rebalance_locked_asset_id']
  end

  test 'a rebalance sell is refused while a buy for the same asset is resting' do
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    resting_buy
    venue_says_resting_buy(:open)
    @bot.stubs(:rebalance_sell_order_data).returns(ticker: @assets['AAA'][:ticker], price: 80, amount: 1.to_d,
                                                   quote_amount: 80.to_d, side: :sell, order_type: :market_order,
                                                   transaction_type: 'REBALANCE')
    @bot.expects(:create_order).never

    result = @bot.send(:start_rebalance!)

    assert_equal :open_buy, result.data[:skipped]
    assert_not_predicate @bot, :rebalance_pending?
  end

  test 'a survivor set with nothing to steer toward is not a rebalance' do
    # Both weighted members are locked and what is left is a parked holding with a zero weight.
    # Selling it would strand the swap in its buying phase — there is no positive shortfall to buy
    # into — and the DCA leg stands down behind a pending rebalance until the lock expires.
    index_membership('AAA' => 0.5, 'BBB' => 0.5, 'CCC' => 0.0)
    @bot.bot_index_assets.where(asset: [@assets['AAA'][:asset], @assets['BBB'][:asset]])
        .update_all(buy_locked_until: 10.days.from_now)
    stub_values({ 'AAA' => 50, 'BBB' => 30, 'CCC' => 20 })

    assert_nil @bot.send(:rebalance_targets)
    assert_nil @bot.rebalance_drift
  end

  test 'a resting buy the venue has since filled does not stand the rebalance down for good' do
    # Rebalancing runs while the DCA schedule is stopped, and on a stopped bot nothing else polls a
    # resting DCA order — a guard that trusted the stale row would block this leg permanently.
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    order = resting_buy
    venue_says_resting_buy(:closed)
    @bot.stubs(:rebalance_sell_order_data).returns(ticker: @assets['AAA'][:ticker], price: 80, amount: 1.to_d,
                                                   quote_amount: 80.to_d, side: :sell, order_type: :market_order,
                                                   transaction_type: 'REBALANCE')
    @bot.stubs(:calculate_best_amount_info).returns(below_minimum_amount: false)
    @bot.stubs(:create_order).returns(Result::Success.new(order_id: 'x'))
    @bot.stubs(:persist_accepted_order!).returns(@bot.transactions.build)
    Bot::FetchAndUpdateOrderJob.stubs(:perform_later)

    result = @bot.send(:start_rebalance!)

    assert_equal 'closed', order.reload.external_status, 'the guard refreshed it'
    assert_nil result.data[:skipped], 'and the sell went out'
  end

  test 'a rebalance sell at a loss that goes out stays locked' do
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    @bot.set_missed_quote_amount
    @bot.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
    @bot.stubs(:metrics).returns(asset_breakdown: {}, asset_lots: { 'AAA' => [{ amount: 2.to_d, cost: 200.to_d }] })
    @bot.stubs(:rebalance_sell_order_data).returns(ticker: @assets['AAA'][:ticker], price: 80, amount: 1.to_d,
                                                   quote_amount: 80.to_d, side: :sell, order_type: :market_order,
                                                   transaction_type: 'REBALANCE')
    @bot.stubs(:calculate_best_amount_info).returns(below_minimum_amount: false)
    @bot.stubs(:create_order).returns(Result::Success.new(order_id: 'x'))
    @bot.stubs(:persist_accepted_order!).returns(@bot.transactions.build)
    Bot::FetchAndUpdateOrderJob.stubs(:perform_later)

    @bot.send(:start_rebalance!)

    assert_predicate @bot.bot_index_assets.find_by(asset: @assets['AAA'][:asset]), :buy_locked?
  end

  test 'a later failed sale never rolls back the lock an earlier, resolved one left' do
    # Sale 1 halts with an unknown outcome and the user clears it by hand; its lock must stand,
    # because that sale may well have executed. Sale 2 is a different name, at a gain, and provably
    # never reaches the venue — its rollback belongs to itself.
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    @bot.set_missed_quote_amount
    @bot.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
    @bot.stubs(:metrics).returns(asset_breakdown: {}, asset_lots: { 'AAA' => [{ amount: 2.to_d, cost: 200.to_d }] })
    @bot.stubs(:calculate_best_amount_info).returns(below_minimum_amount: false)
    @bot.stubs(:rebalance_sell_order_data).returns(ticker: @assets['AAA'][:ticker], price: 80, amount: 1.to_d,
                                                   quote_amount: 80.to_d, side: :sell, order_type: :market_order,
                                                   transaction_type: 'REBALANCE')
    @bot.stubs(:create_order).raises(Client::AmbiguousPlacementError.new('timeout'))
    @bot.send(:start_rebalance!)
    locked_until = @bot.bot_index_assets.find_by(asset: @assets['AAA'][:asset]).buy_locked_until
    assert locked_until, 'the ambiguous sale locked the name'
    @bot.clear_rebalance_pending! # what the resolution controller does

    @bot.stubs(:rebalance_sell_order_data).returns(ticker: @assets['BBB'][:ticker], price: 200, amount: 1.to_d,
                                                   quote_amount: 200.to_d, side: :sell, order_type: :market_order,
                                                   transaction_type: 'REBALANCE')
    @bot.stubs(:create_order).raises(Client::TransientNetworkError.new('dns'))
    @bot.send(:start_rebalance!)

    assert_equal locked_until, @bot.bot_index_assets.find_by(asset: @assets['AAA'][:asset]).buy_locked_until
  end

  test 'a rebalance sell of units whose cost we never learned is locked all the same' do
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    @bot.set_missed_quote_amount
    @bot.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
    @bot.stubs(:metrics).returns(asset_breakdown: {}, asset_lots: { 'AAA' => [{ amount: 2.to_d, cost: nil }] })
    @bot.stubs(:rebalance_sell_order_data).returns(ticker: @assets['AAA'][:ticker], price: 110, amount: 1.to_d,
                                                   quote_amount: 110.to_d, side: :sell, order_type: :market_order,
                                                   transaction_type: 'REBALANCE')
    @bot.stubs(:calculate_best_amount_info).returns(below_minimum_amount: false)
    @bot.stubs(:create_order).returns(Result::Success.new(order_id: 'x'))
    @bot.stubs(:persist_accepted_order!).returns(@bot.transactions.build)
    Bot::FetchAndUpdateOrderJob.stubs(:perform_later)

    @bot.send(:start_rebalance!)

    assert_predicate @bot.bot_index_assets.find_by(asset: @assets['AAA'][:asset]), :buy_locked?
  end

  private

  def resting_buy
    @bot.transactions.create!(exchange: @bot.exchange, base: 'AAA', quote: @bot.quote_asset.symbol, side: :buy,
                              transaction_type: 'REGULAR', status: :submitted, external_status: :open,
                              external_id: 'resting', amount: 1, price: 80, order_type: :limit_order)
  end

  def venue_says_resting_buy(status)
    @bot.stubs(:get_orders).returns(Result::Success.new(
                                      orders: { 'resting' => { status: status, price: 80, amount: 1, quote_amount: 80,
                                                               amount_exec: status == :closed ? 1 : 0,
                                                               quote_amount_exec: status == :closed ? 80 : 0,
                                                               ticker: @assets['AAA'][:ticker], side: :buy,
                                                               order_type: :limit_order } },
                                      missing: []
                                    ))
  end

  def enable_rebalancing(threshold: 0.05)
    @bot.settings = @bot.settings.merge('rebalance_enabled' => true, 'rebalance_threshold' => threshold)
    @bot.set_missed_quote_amount
    @bot.save!
    @assets.each_value do |a|
      stub_ticker_bid_price(a[:ticker], price: 100)
      stub_ticker_ask_price(a[:ticker], price: 100)
    end
    @bot.exchange.stubs(:market_sell).returns(Result::Success.new(order_id: "s-#{SecureRandom.hex(4)}"))
    @bot.exchange.stubs(:market_buy).returns(Result::Success.new(order_id: "b-#{SecureRandom.hex(4)}"))
    balances = @assets.values.to_h { |a| [a[:asset].id, { free: 100.0, locked: 0 }] }
    balances[@bot.quote_asset_id] = { free: 10_000, locked: 0 }
    stub_exchange_balances(@bot.exchange, balances)
  end

  def index_membership(weights)
    weights.each do |symbol, weight|
      BotIndexAsset.create!(bot: @bot, asset: @assets[symbol][:asset], ticker: @assets[symbol][:ticker],
                            target_allocation: weight, in_index: true, entered_at: Time.current)
    end
  end

  def exited(symbol)
    BotIndexAsset.create!(bot: @bot, asset: @assets[symbol][:asset], ticker: @assets[symbol][:ticker],
                          target_allocation: nil, in_index: false, exited_at: Time.current)
  end

  def stub_values(values, stale: false)
    asset_values = values.transform_values { |v| { amount: v.to_d / 100, current_value: v.to_d } }
    @bot.stubs(:metrics_with_current_prices).returns(
      asset_values: asset_values,
      prices_stale: stale
    )
  end
end
