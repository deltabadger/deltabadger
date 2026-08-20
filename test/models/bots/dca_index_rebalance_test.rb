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

  test 'an asset that has left the index carries target zero' do
    # Not an oversight — target 0 is what makes rebalancing liquidate it. Tracking an index means
    # following its composition changes, and the DCA leg already stopped buying this one.
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    exited('CCC')
    stub_values({ 'AAA' => 40, 'BBB' => 40, 'CCC' => 20 })

    targets = @bot.send(:rebalance_targets).index_by { |t| t[:ticker].base }

    assert_in_delta 0, targets['CCC'][:target].to_f, 0.0001
  end

  test 'an exited holding still counts toward the portfolio total' do
    # Leaving it out would understate the portfolio and make every other asset look overweight.
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    exited('CCC')
    stub_values({ 'AAA' => 40, 'BBB' => 40, 'CCC' => 20 })

    total = @bot.send(:rebalance_targets).sum { |t| t[:value] }

    assert_in_delta 100, total.to_f, 0.0001
  end

  test 'drift is the largest single-asset deviation, not an average' do
    index_membership('AAA' => 0.34, 'BBB' => 0.33, 'CCC' => 0.33)
    stub_values({ 'AAA' => 60, 'BBB' => 20, 'CCC' => 20 })

    # AAA is 60 % against a 34 % target: 26 points. The others are 13 points under.
    assert_in_delta 0.26, @bot.rebalance_drift.to_f, 0.001
  end

  test 'an exited holding drives drift by its whole weight' do
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    exited('CCC')
    stub_values({ 'AAA' => 45, 'BBB' => 45, 'CCC' => 10 })

    # CCC is 10 % of the portfolio against a 0 % target.
    assert_in_delta 0.10, @bot.rebalance_drift.to_f, 0.001
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

  test 'an exited asset outranks a merely overweight one and is sold first' do
    # Its whole holding is excess, so it is almost always the largest single correction available.
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    exited('CCC')
    stub_values({ 'AAA' => 34, 'BBB' => 26, 'CCC' => 40 })
    enable_rebalancing

    @bot.rebalance!

    assert_equal 'CCC', @bot.transactions.last.base
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
    # Its target is 0, so its deviation is its entire value — always positive, never the minimum.
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
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    exited('CCC')
    @assets['CCC'][:ticker].update!(available: false)
    stub_values({ 'AAA' => 70, 'BBB' => 10, 'CCC' => 20 })
    enable_rebalancing

    @bot.rebalance!

    assert_equal 'AAA', @bot.transactions.last.base, 'the rest of the portfolio still rebalances'
  end

  test 'an in-index asset with no weight is treated as on target, not as a liquidation' do
    # Only LEAVING the index means target 0. A missing number must never be read as "sell it all".
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

    @bot.expects(:refresh_index_composition).returns(Result::Success.new)
    @bot.rebalance!
  end

  test 'stale prices stop the index bot too' do
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
    stub_values({ 'AAA' => 90, 'BBB' => 10 }, stale: true)
    enable_rebalancing

    assert_nil @bot.rebalance_drift
    assert_not @bot.rebalance_due?
  end

  private

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
