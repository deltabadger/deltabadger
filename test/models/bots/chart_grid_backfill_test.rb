# frozen_string_literal: true

require 'test_helper'

# chart_marked_at_market drops a point outright when a single HELD asset cannot be priced there.
# On a basket that makes one member's candle coverage the whole chart's: a member whose fetch
# failed, or whose candles begin after the bot started holding it, takes the plot down to the
# transaction times — a shape with a point per purchase and nothing between them.
#
# So a symbol the candles do not span is backfilled from the bot's own fills. Those are real
# observed prices covering exactly the period the asset was held.
class ChartGridBackfillTest < ActiveSupport::TestCase
  T0 = Time.utc(2026, 1, 1, 12, 0, 0)
  T1 = T0 + 2.days
  NOW = T0 + 14.days

  setup do
    travel_to NOW
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @bot = create(:dca_multi_asset, user: create(:user))
    @btc, @eth = @bot.base_assets
  end

  teardown { travel_back }

  def record_fill!(asset, time, price)
    @bot.transactions.create!(
      exchange: @bot.exchange, base: asset.symbol, quote: @bot.quote_asset.symbol,
      side: :buy, status: :submitted, external_status: :closed,
      amount: 1, amount_exec: 1, quote_amount: price, quote_amount_exec: price,
      price: price, created_at: time, external_id: "fill-#{asset.symbol}-#{time.to_i}"
    )
  end

  def daily_marks(price, from:, to: NOW)
    day = from.midnight
    marks = []
    while day <= to
      marks << [day, price.to_d]
      day += 1.day
    end
    marks
  end

  def backfilled(grids)
    @bot.send(:chart_backfilled_grids, grids, symbols: %w[BTC ETH], from: T0, to: NOW)
  end

  test 'a grid that already spans the window is left exactly as it was' do
    grids = { 'BTC' => daily_marks(100, from: T0 - 1.day), 'ETH' => daily_marks(50, from: T0 - 1.day) }
    before = grids.transform_values(&:dup)

    assert_equal before, backfilled(grids)
  end

  test 'a symbol with no grid at all is given its own fills' do
    record_fill!(@eth, T0, 50)
    record_fill!(@eth, T1, 60)
    grids = { 'BTC' => daily_marks(100, from: T0 - 1.day) }

    result = backfilled(grids)

    assert_equal([[T0, 50], [T1, 60]], result['ETH'].map { |time, price| [time, price.to_i] })
  end

  test 'a grid that starts after the bot began holding is extended back to the first fill' do
    record_fill!(@eth, T0, 50)
    late = daily_marks(50, from: T0 + 7.days)
    grids = { 'BTC' => daily_marks(100, from: T0 - 1.day), 'ETH' => late }

    result = backfilled(grids)

    assert_equal T0, result['ETH'].first[0]
    assert_operator result['ETH'].size, :>, late.size
  end

  test 'the venue reading wins where a candle and a fill share a timestamp' do
    record_fill!(@eth, T0.midnight, 999)
    grids = { 'BTC' => daily_marks(100, from: T0 - 1.day), 'ETH' => daily_marks(50, from: T0 + 7.days) }

    result = backfilled(grids)
    at_midnight = result['ETH'].select { |time, _| time == T0.midnight }

    assert_equal 1, at_midnight.size
    assert_equal 999, at_midnight.first[1].to_i
  end

  test 'a symbol with neither candles nor fills is left without a grid rather than invented' do
    grids = { 'BTC' => daily_marks(100, from: T0 - 1.day) }

    assert_nil backfilled(grids)['ETH']
  end

  test 'the backfilled marks stay sorted, so the grid can still be searched' do
    record_fill!(@eth, T0 + 3.days, 55)
    record_fill!(@eth, T0 + 1.day, 50)
    grids = { 'ETH' => daily_marks(50, from: T0 + 7.days) }

    times = backfilled(grids)['ETH'].map(&:first)

    assert_equal times.sort, times
  end

  # == Which fills mark a price ==

  test 'a fill inside the span the candles cover never replaces their reading' do
    record_fill!(@btc, T0 + 1.day, 90) # before the candles: kept
    record_fill!(@btc, T1 + 7.hours, 50) # between two candles: the venue's reading stands

    grid = backfilled('BTC' => daily_marks(100, from: T1), 'ETH' => daily_marks(10, from: T0))['BTC']

    assert_includes grid, [T0 + 1.day, 90.to_d]
    assert_not(grid.any? { |time, _price| time == T1 + 7.hours })
  end

  test 'an order that executed nothing marks nothing' do
    record_fill!(@btc, T0 + 1.day, 90)
    record_fill!(@btc, T0 + 1.day + 3.hours, 50).update_columns(external_status: Transaction.external_statuses[:open],
                                                                amount_exec: nil, quote_amount_exec: nil)

    grid = backfilled('ETH' => daily_marks(10, from: T0))['BTC']

    assert_equal [[T0 + 1.day, 90.to_d]], grid
  end

  test 'a sale that reported neither its executed amount nor its proceeds marks nothing' do
    record_fill!(@btc, T0 + 1.day, 90)
    record_fill!(@btc, T0 + 1.day + 3.hours, 50).update_columns(side: Transaction.sides[:sell], amount_exec: nil,
                                                                quote_amount_exec: nil)

    grid = backfilled('ETH' => daily_marks(10, from: T0))['BTC']

    assert_equal [[T0 + 1.day, 90.to_d]], grid
  end

  test 'of two fills at one moment the later one stands' do
    record_fill!(@btc, T0 + 1.day, 100)
    @bot.transactions.create!(exchange: @bot.exchange, base: @btc.symbol, quote: @bot.quote_asset.symbol, side: :buy,
                              status: :submitted, external_status: :closed, amount: 1, amount_exec: 1, quote_amount: 80,
                              quote_amount_exec: 80, price: 80, created_at: T0 + 1.day, external_id: 'fill-later')

    grid = backfilled('ETH' => daily_marks(10, from: T0))['BTC']

    assert_equal [[T0 + 1.day, 80.to_d]], grid
  end
end
