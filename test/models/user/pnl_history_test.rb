# frozen_string_literal: true

require 'test_helper'

# Contract for the dashboard's mini P/L curve.
#
# The headline on /bots is a POINT — what the bots are up right now. This is the same figure over
# time, read off the CANDLE-MARKED metrics: the source the bot's own chart uses, where the value
# between two purchases is the holding priced at market rather than a flat step from fill to fill.
# Nothing warms that cache in the background, so a cold one is a wait — `loading` — and the page
# asks for the live pass exactly as it does for a cold headline.
#
# The tracker's curve is the whole account (every connected venue, every manual holding); this one
# is the BOTS alone, so the two are allowed to disagree.
class User::PnlHistoryTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @now = Time.current
  end

  # The metrics cache, in the shape every measurable bot stores it: labels, then value and
  # invested as two parallel rows.
  def metrics(points)
    { chart: { labels: points.map { |at, _, _| at },
               series: [points.map { |_, value, _| value }, points.map { |_, _, invested| invested }] } }
  end

  # Stubbed per CLASS, which is why the two-bot case uses two different bot types: the bots come
  # off an association, so there is no instance here to stub.
  def marked(type, points)
    type.any_instance.stubs(:metrics_with_current_prices_and_candles_from_cache)
        .returns(points && metrics(points))
  end

  # A column per reading, plus one in FRONT of them all: an account has made nothing before its
  # first purchase, and the curve starts there rather than on whatever that first fill marks — a
  # just-bought position priced against a candle grid opens a spread below what was paid for it.
  test 'a column per reading, opening on the zero line' do
    marked(Bots::DcaSingleAsset, [[@now - 2.days, 100, 100], [@now - 1.day, 110, 100], [@now, 90, 100]])
    create(:dca_single_asset, user: @user)

    history = User::PnlHistory.snapshot(@user)[:result]

    assert_in_delta 2.0, history[:days], 1e-6, 'the span is the history, not the run-up to it'
    assert_equal([0.0, 0.0, 0.1, -0.1], history[:percent].map { |value| value.round(6) })
    assert_equal([0.0, 0.0, 10.0, -10.0], history[:profit_usd].map { |value| value.round(6) })
  end

  # The marked series carries the market between purchases, so a bot with no reading in a column
  # is not flat by accident — it is flat because that is what it was last worth.
  test 'a bot with no reading in a column keeps its last one, and the bots are summed' do
    btc = create(:asset, :bitcoin)
    usd = create(:asset, :usd)
    single = create(:dca_single_asset, user: @user, base_asset: btc, quote_asset: usd)
    create(:dca_multi_asset, user: @user, exchange: single.exchange, quote_asset: usd,
                             base_assets: [btc, create(:asset, :ethereum)])
    marked(Bots::DcaSingleAsset, [[@now - 2.days, 100, 100], [@now, 120, 100]])
    marked(Bots::DcaMultiAsset, [[@now - 1.day, 50, 50]])

    history = User::PnlHistory.snapshot(@user)[:result]

    # Three readings a day apart, behind the opening column. First: nothing yet. Second: the
    # single-asset bot alone (100/100). Third: it holds while the other arrives (150/150).
    # Fourth: it moves to 120 and the other holds (170/150).
    assert_equal 4, history[:percent].size
    assert_equal([0.0, 0.0, 0.0, 0.133333], history[:percent].map { |value| value.round(6) })
    assert_equal([0.0, 0.0, 0.0, 20.0], history[:profit_usd].map { |value| value.round(6) })
  end

  test 'a non-USD quote is converted at the cached rate' do
    eur = create(:asset, :eur)
    create(:dca_single_asset, user: @user, quote_asset: eur)
    marked(Bots::DcaSingleAsset, [[@now - 1.day, 100, 100], [@now, 120, 100]])
    Utilities::Currency.stubs(:exchange_rate)
                       .with(from: 'EUR', to: 'USD', cache_only: true)
                       .returns(Result::Success.new(1.1))

    history = User::PnlHistory.snapshot(@user)[:result]

    # A ratio is rate-free; the money is not.
    assert_equal([0.0, 0.0, 0.2], history[:percent].map { |value| value.round(6) })
    assert_equal([0.0, 0.0, 22.0], history[:profit_usd].map { |value| value.round(6) })
  end

  test 'a cold FX rate draws nothing, and says so' do
    eur = create(:asset, :eur)
    create(:dca_single_asset, user: @user, quote_asset: eur)
    marked(Bots::DcaSingleAsset, [[@now - 1.day, 100, 100], [@now, 120, 100]])

    snapshot = User::PnlHistory.snapshot(@user)

    assert_nil snapshot[:result]
    assert_equal true, snapshot[:loading]
  end

  # The marked metrics are the heavy cache and nothing warms them in the background. A bot that
  # HAS a history is worth waiting for; the cheap hash is what knows whether it has one.
  test 'a bot with history but no marked reading yet is a wait, not an empty chart' do
    create(:dca_single_asset, user: @user)
    marked(Bots::DcaSingleAsset, nil)
    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices_from_cache)
                        .returns(metrics([[@now - 1.day, 100, 100], [@now, 120, 100]]))

    snapshot = User::PnlHistory.snapshot(@user)

    assert_nil snapshot[:result]
    assert_equal true, snapshot[:loading]
  end

  test 'a bot that has never traded is not something to wait for' do
    create(:dca_single_asset, user: @user)
    marked(Bots::DcaSingleAsset, nil)
    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices_from_cache).returns(nil)

    snapshot = User::PnlHistory.snapshot(@user)

    assert_nil snapshot[:result]
    assert_equal false, snapshot[:loading]
  end

  # A live pass may compute what a request may only read — this is what the broadcast job does.
  test 'a live pass computes the marked metrics instead of waiting for them' do
    create(:dca_single_asset, user: @user)
    marked(Bots::DcaSingleAsset, nil)
    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices_and_candles)
                        .returns(metrics([[@now - 1.day, 100, 100], [@now, 120, 100]]))

    history = User::PnlHistory.snapshot(@user, live: true)[:result]

    assert_equal([0.0, 0.0, 0.2], history[:percent].map { |value| value.round(6) })
  end

  test 'a single moment is not a curve' do
    create(:dca_single_asset, user: @user)
    marked(Bots::DcaSingleAsset, [[@now, 120, 100]])

    assert_nil User::PnlHistory.snapshot(@user)[:result]
  end

  # The plot spaces its points evenly, so the readings are resampled onto an even axis: a curve
  # built from raw readings would stretch a quiet week and squeeze a busy one.
  test 'a long history is resampled onto an even axis, ending on the live reading' do
    points = 400.times.map { |i| [@now - (399 - i).days, 100 + i, 100] }
    create(:dca_single_asset, user: @user)
    marked(Bots::DcaSingleAsset, points)

    history = User::PnlHistory.snapshot(@user)[:result]

    assert_in_delta 399.0, history[:days], 1e-6
    assert_equal User::PnlHistory::MAX_POINTS, history[:percent].size
    assert_equal 0.0, history[:percent].first.round(6)
    assert_equal 3.99, history[:percent].last.round(6), 'the last column is the last reading'
    assert_equal history[:percent].size, history[:profit_usd].size
  end

  test 'a deleted bot is not part of the account curve' do
    bot = create(:dca_single_asset, user: @user)
    marked(Bots::DcaSingleAsset, [[@now - 1.day, 100, 100], [@now, 120, 100]])
    bot.update!(status: :deleted)

    assert_nil User::PnlHistory.snapshot(@user)[:result]
  end
end
