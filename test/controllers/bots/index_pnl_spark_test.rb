# frozen_string_literal: true

require 'test_helper'

# The dashboard's P/L curve, on the page.
#
# It is drawn in the same element the headline lives in, because that element is what the P/L
# broadcast replaces — a curve outside it would go on drawing an old history under a fresh number.
# Everything it needs to answer a hover ships with it: the two series, the scale the server drew it
# at, and the rate the amounts are shown in.
class Bots::IndexPnlSparkTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true) # satisfies the onboarding gate (an admin must exist)
    @user = create(:user)
    exchange = create(:binance_exchange)
    shared = { user: @user, exchange: exchange, base_asset: create(:asset, :bitcoin),
               quote_asset: create(:asset, :usd) }
    create(:dca_single_asset, **shared)
    create(:dca_single_asset, **shared) # a second bot: a lone bot redirects to its own page

    User.any_instance.stubs(:global_pnl_snapshot).returns(
      { result: { percent: 0.25.to_d, profit_usd: 25.to_d }, loading: false }
    )
    sign_in @user
  end

  # The curve is drawn from the CANDLE-MARKED metrics — the bot chart's own ruler — so that is
  # what a warm dashboard has cached.
  def stub_history(points)
    metrics = { pnl: 0.25.to_d, total_quote_amount_invested: 100.to_d, total_amount_value_in_quote: 125.to_d,
                chart: { labels: points.map { |at, _, _| at },
                         series: [points.map { |_, value, _| value }, points.map { |_, _, invested| invested }] } }
    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices_from_cache).returns(metrics)
    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices_and_candles_from_cache).returns(metrics)
  end

  # The plot is drawn on a later trip than the page, so the section holds its height from the first
  # paint — otherwise it grows when the curve lands and pushes the whole dashboard down.
  test 'a section that will hold a plot reserves its height before the curve is there' do
    now = Time.current
    metrics = { pnl: 0.25.to_d, total_quote_amount_invested: 100.to_d, total_amount_value_in_quote: 125.to_d,
                chart: { labels: [now - 1.day, now], series: [[100, 125], [100, 100]] } }
    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices_from_cache).returns(metrics)
    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices_and_candles_from_cache).returns(nil)

    get bots_path

    assert_select '.dash-intro.dash-intro--plotted'
    assert_select '.dash-intro__spark', false, 'the curve itself is still on its way'
  end

  test 'a section with the curve already in it keeps the same reserved height' do
    now = Time.current
    stub_history([[now - 1.day, 100, 100], [now, 125, 100]])

    get bots_path

    assert_select '.dash-intro.dash-intro--plotted .dash-intro__spark'
  end

  test 'a month of history draws a full-width curve on the zero line' do
    now = Time.current
    stub_history(31.times.map { |day| [now - (30 - day).days, 100 + day, 100] })

    get bots_path

    assert_select '#global-pnl[data-controller=?]', 'pnl-spark'
    assert_select '.dash-intro__spark__curve[style=?]', 'width: 100.0%'
    assert_select '.dash-intro__spark__line'
  end

  test 'a fortnight of history draws half of it' do
    now = Time.current
    stub_history(16.times.map { |day| [now - (15 - day).days, 100 + day, 100] })

    get bots_path

    assert_select '.dash-intro__spark__curve[style=?]', 'width: 50.0%'
  end

  test 'the plot is the same box above and below the line, whatever the curve did' do
    now = Time.current
    stub_history([[now - 2.days, 100, 100], [now - 1.day, 90, 100], [now, 125, 100]])

    get bots_path

    assert_select '.dash-intro__spark:not([style])'
    assert_select '.dash-intro__spark svg[viewBox=?]', '0 0 100 200'
  end

  test 'the headline still reads as it did, and the figures are the hover targets' do
    now = Time.current
    stub_history([[now - 1.day, 100, 100], [now, 125, 100]])

    get bots_path

    assert_select '#global-pnl .pnl-percent[data-pnl-spark-target=?]', 'percent', text: /\+25\.00%/
    assert_select '#global-pnl .pnl-amount[data-pnl-spark-target=?]', 'amount', text: /\+\$25/
  end

  # The figure the pointer replaces was grouped by Rails. The browser groups a Polish page with
  # spaces, so the mark travels with the data rather than being read off <html lang>.
  test 'the hover carries the thousands mark the server wrote with' do
    now = Time.current
    stub_history([[now - 1.day, 100, 100], [now, 125, 100]])

    get bots_path

    assert_select '#global-pnl[data-pnl-spark-delimiter-value=?]', ','
  end

  test 'no history, no curve — and the headline is untouched' do
    metrics = { pnl: 0.25.to_d, total_quote_amount_invested: 100.to_d, total_amount_value_in_quote: 125.to_d }
    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices_from_cache).returns(metrics)
    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices_and_candles_from_cache).returns(metrics)

    get bots_path

    assert_select '.dash-intro__spark', false
    assert_select '#global-pnl .pnl-percent', text: /\+25\.00%/
    assert_select '.dash-intro[data-controller]', false, 'nothing to wait for, so nothing is asked for'
    assert_select '.dash-intro--plotted', false, 'and no space held for a plot that is not coming'
  end

  # The marked metrics are not warmed in the background, so a dashboard that finds them cold asks
  # for the live pass — the same one a cold headline asks for, rendering both.
  test 'a cold curve asks for the pass that fills it, without claiming the headline is loading' do
    now = Time.current
    metrics = { pnl: 0.25.to_d, total_quote_amount_invested: 100.to_d, total_amount_value_in_quote: 125.to_d,
                chart: { labels: [now - 1.day, now], series: [[100, 125], [100, 100]] } }
    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices_from_cache).returns(metrics)
    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices_and_candles_from_cache).returns(nil)

    get bots_path

    assert_select '.dash-intro[data-broadcast--on-connect-method-value=?]', 'global_pnl_update'
    assert_select '.dash-intro[data-broadcast--on-connect-retry-while-value]', false
    assert_select '.dash-intro__spark', false
    assert_select '#global-pnl .pnl-percent', text: /\+25\.00%/
  end

  test 'hiding balances keeps the curve and takes the amount' do
    @user.update!(hide_balances: true)
    now = Time.current
    stub_history([[now - 1.day, 100, 100], [now, 125, 100]])

    get bots_path

    assert_select '.dash-intro__spark__line'
    assert_select '#global-pnl .pnl-amount', false
    # The ratio is not a balance; the profit behind it is, and an attribute is not a hiding place.
    assert_select '#global-pnl[data-pnl-spark-percent-value]'
    assert_select '#global-pnl[data-pnl-spark-profit-value]', false
  end
  # The wait is the axis, not a spinner — and the poll that retries it has to be looking for the
  # same element, or a cold headline never asks again.
  test 'a cold headline waits on the zero line, and the retry watches that line' do
    User.any_instance.stubs(:global_pnl_snapshot).returns({ result: nil, loading: true })

    get bots_path

    assert_select '#global-pnl .dash-intro__loading'
    assert_select '#global-pnl .loader--small', false, 'no spinner in the headline any more'
    assert_select '.dash-intro[data-broadcast--on-connect-retry-while-value=?]',
                  '#global-pnl .dash-intro__loading'
  end

end
