require 'test_helper'

class Bots::DcaMultiAssetCompositionTest < ActiveSupport::TestCase
  include Turbo::Broadcastable::TestHelper

  def setup
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @quote = create(:asset, :usd)
    @assets = %w[AAA BBB CCC].to_h do |symbol|
      asset = create(:asset, symbol: symbol, name: "Coin #{symbol}", external_id: "coin-#{symbol.downcase}")
      ticker = create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: @quote)
      [symbol, { asset:, ticker: }]
    end
  end

  test 'saving the bot writes one in-index row per member at the stored weight' do
    bot = create_bot('AAA' => 0.5, 'BBB' => 0.3, 'CCC' => 0.2)

    rows = bot.bot_index_assets.in_index.index_by { it.asset.symbol }
    assert_equal %w[AAA BBB CCC], rows.keys.sort
    assert_in_delta 0.5, rows['AAA'].target_allocation, 0.0001
    assert_in_delta 0.3, rows['BBB'].target_allocation, 0.0001
    assert_in_delta 0.2, rows['CCC'].target_allocation, 0.0001
    assert rows.values.all?(&:entered_at?)
  end

  test 'a member with no tradeable ticker is left out and the rest are renormalised' do
    bot = create_bot('AAA' => 0.4, 'BBB' => 0.4, 'CCC' => 0.2)
    @assets['CCC'][:ticker].update!(available: false)

    result = bot.refresh_composition

    assert_predicate result, :success?
    rows = bot.bot_index_assets.in_index.index_by { it.asset.symbol }
    assert_equal %w[AAA BBB], rows.keys.sort
    assert_in_delta 0.5, rows['AAA'].target_allocation, 0.0001
    assert_in_delta 0.5, rows['BBB'].target_allocation, 0.0001
    assert_predicate bot.bot_index_assets.find_by(asset: @assets['CCC'][:asset]), :exited_at?
  end

  test 'only zero-weight assets tradeable: refresh fails and rows stay' do
    bot = create_bot('AAA' => 1.0, 'BBB' => 0.0, 'CCC' => 0.0)
    before = bot.bot_index_assets.in_index.order(:asset_id).pluck(:asset_id, :target_allocation)
    @assets['AAA'][:ticker].update!(available: false)

    result = bot.refresh_composition

    assert_predicate result, :failure?
    assert_equal before, bot.bot_index_assets.in_index.order(:asset_id).pluck(:asset_id, :target_allocation)
  end

  test 'removing an asset marks its row exited, synchronously, and keeps it as a holding' do
    bot = create_bot('AAA' => 0.5, 'BBB' => 0.25, 'CCC' => 0.25)
    bot.assign_attributes(bot.parse_params(remove_asset_id: @assets['BBB'][:asset].id))
    bot.set_missed_quote_amount
    bot.save!

    row = bot.bot_index_assets.find_by!(asset: @assets['BBB'][:asset])
    assert_not row.in_index?
    assert_predicate row, :exited_at?

    values = {
      'AAA' => { amount: 0.5.to_d, current_value: 50.to_d },
      'BBB' => { amount: 0.25.to_d, current_value: 25.to_d },
      'CCC' => { amount: 0.25.to_d, current_value: 25.to_d }
    }
    bot.stubs(:metrics_with_current_prices).returns(asset_values: values)
    bot.stubs(:metrics).returns(asset_breakdown: values)

    assert_equal(['BBB'], bot.exited_holdings.map { it[:symbol] })
    assert_equal ['BBB'], bot.exited_symbols
  end

  test 'the metrics panel is broadcast after a composition change and not after an amount change' do
    bot = create_bot('AAA' => 0.5, 'BBB' => 0.5)
    bot.stubs(:metrics_with_current_prices).returns(
      realised_pnl: 0,
      prices_stale: false,
      total_quote_amount_invested: 0,
      total_amount_value_in_quote: 0,
      asset_values: {}
    )
    stream = ["user_#{bot.user_id}", :bot_updates]

    broadcasts = capture_turbo_stream_broadcasts(stream) do
      bot.allocations = { @assets['AAA'][:asset].id.to_s => 0.6, @assets['BBB'][:asset].id.to_s => 0.4 }
      bot.set_missed_quote_amount
      bot.save!
    end
    assert_equal(1, broadcasts.count { it['target'] == 'metrics' })

    broadcasts = capture_turbo_stream_broadcasts(stream) do
      bot.quote_amount = 125
      bot.set_missed_quote_amount
      bot.save!
    end
    assert_equal(0, broadcasts.count { it['target'] == 'metrics' })
  end

  test 're-adding a removed asset brings its row back' do
    bot = create_bot('AAA' => 0.5, 'BBB' => 0.25, 'CCC' => 0.25)
    bot.allocations = bot.allocations_removing(@assets['CCC'][:asset].id)
    bot.set_missed_quote_amount
    bot.save!
    assert_not bot.bot_index_assets.find_by!(asset: @assets['CCC'][:asset]).in_index?

    bot.allocations = bot.allocations_adding(@assets['CCC'][:asset].id)
    bot.set_missed_quote_amount
    bot.save!

    row = bot.bot_index_assets.find_by!(asset: @assets['CCC'][:asset])
    assert_predicate row, :in_index?
    assert_nil row.exited_at
  end

  test 'rebalance_targets come from the in-index rows with the user weights' do
    bot = create_bot('AAA' => 0.5, 'BBB' => 0.3, 'CCC' => 0.2)
    bot.allocations = bot.allocations_removing(@assets['CCC'][:asset].id)
    bot.set_missed_quote_amount
    bot.save!
    bot.stubs(:metrics_with_current_prices).returns(
      asset_values: {
        'AAA' => { amount: 0.5.to_d, current_value: 50.to_d },
        'BBB' => { amount: 0.3.to_d, current_value: 30.to_d },
        'CCC' => { amount: 0.2.to_d, current_value: 20.to_d }
      },
      asset_breakdown: {},
      prices_stale: false
    )

    targets = bot.send(:rebalance_targets).index_by { it[:ticker].base }

    assert_equal %w[AAA BBB], targets.keys.sort
    assert_in_delta 0.625, targets['AAA'][:target], 0.0001
    assert_in_delta 0.375, targets['BBB'][:target], 0.0001
  end

  private

  def create_bot(weights)
    allocations = weights.to_h { |symbol, weight| [@assets.fetch(symbol)[:asset], weight] }
    create(:dca_multi_asset, user: @user, exchange: @exchange,
                             base_assets: allocations.keys, quote_asset: @quote, allocations:)
  end
end
