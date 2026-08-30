require 'test_helper'

class BotHelperTest < ActionView::TestCase
  # The headline labels below are written with it, exactly as the view has it.
  include NumbersHelper

  test 'whitelist ip comes from the claimed exchange proxy' do
    AppConfig.set('proxy_binance', 'http://user:secret@claimed-proxy.test:9000')

    assert_equal 'claimed-proxy.test', whitelist_ip_for('binance')
  end

  # Regression: a pending (open, unfilled) limit order has amount_exec == 0.0 (not nil),
  # which broke the old `amount_exec || amount` fallback and rendered "Buying 0.0 X for 0.0 Y".
  # It should now show the requested amounts with open-order wording.
  test 'pending limit buy shows open-order wording with requested (not executed) amounts' do
    order = build(:transaction, side: :buy, status: :submitted, external_status: :open,
                                base: 'CKB', quote: 'USDC',
                                amount: 7107, amount_exec: 0, quote_amount: 10, quote_amount_exec: 0)

    assert_equal 'Open order to buy 7107.0 CKB for 10.00 USDC', transaction_summary(order)
  end

  test 'pending limit sell shows open-order sell wording with requested amounts' do
    order = build(:transaction, side: :sell, status: :submitted, external_status: :open,
                                base: 'CKB', quote: 'USDC',
                                amount: 7107, amount_exec: 0, quote_amount: 10, quote_amount_exec: 0)

    assert_equal 'Open order to sell 7107.0 CKB for 10.00 USDC', transaction_summary(order)
  end

  test 'filled buy shows bought wording with executed amounts' do
    order = build(:transaction, side: :buy, status: :submitted, external_status: :closed,
                                base: 'CKB', quote: 'USDC',
                                amount: 7107, amount_exec: 7107, quote_amount: 10, quote_amount_exec: 9.99)

    assert_equal 'Bought 7107.0 CKB for 9.99 USDC', transaction_summary(order)
  end

  # == money is written to the cent ==
  #
  # The venue publishes whatever precision it likes for a pair, which left a stablecoin column
  # reading 10.0 next to 9.99 next to 0.001407. A quantity of a coin still keeps the venue's
  # decimals: two places there would round most tokens away entirely.
  test 'a stablecoin amount is written to the cent whatever the venue publishes' do
    assert_equal '10.00', round_amount(10.to_d, 8, 'USDC')
    assert_equal '9.99', round_amount(9.99.to_d, 8, 'USDT')
  end

  test 'a fiat amount is written to the cent' do
    assert_equal '1234.50', round_amount(1234.5.to_d, 6, 'EUR')
  end

  test 'a coin amount keeps the decimals the venue publishes' do
    assert_equal 0.001407.to_d, round_amount(0.0014069999.to_d, 6, 'BTC')
  end

  test 'no currency named falls back to the venue decimals' do
    assert_equal 1.23.to_d, round_amount(1.2345.to_d, 2, nil)
  end

  # == abandoned external_status ==

  test 'abandoned order reuses cancelled summary copy' do
    order = build(:transaction, status: :submitted, external_status: :abandoned,
                                base: 'CKB', quote: 'USDC',
                                amount: 100, quote_amount: 5, external_id: 'a1')

    assert_equal t('bot_activity.transactions.cancelled'), transaction_summary(order)
  end

  test 'bot_activity_summary returns a meaningful English string for order_abandoned' do
    activity = BotActivityLog.new(event: 'order_abandoned', level: :info, details: { 'order_id' => 'OABC123' })

    I18n.with_locale(:en) do
      summary = bot_activity_summary(activity)

      refute_nil summary
      refute_match(/translation missing/i, summary)
      assert_match(/abandoned|no longer/i, summary,
                   'expected the order_abandoned event to render copy that names the abandonment')
    end
  end

  # == order_filter_type — used by _order.html.erb to tag the row with its tab ==

  test 'order_filter_type maps open and unknown to the waiting tab' do
    bot = create(:dca_single_asset)
    assert_equal 'waiting',
                 order_filter_type(build(:transaction, bot: bot, status: :submitted, external_status: :open, external_id: 'o1'))
    assert_equal 'waiting',
                 order_filter_type(build(:transaction, bot: bot, status: :submitted, external_status: :unknown, external_id: 'u1'))
  end

  test 'order_filter_type maps closed to the successful tab' do
    assert_equal 'successful',
                 order_filter_type(build(:transaction, status: :submitted, external_status: :closed, external_id: 'cl1'))
  end

  test 'order_filter_type is nil for every row the Other tab owns' do
    # Cancelled, abandoned, skipped and failed rows are shown as sentence rows under
    # "Other", so their columnar row belongs to no tab and must never leak into
    # Transactions or Scheduled.
    bot = create(:dca_single_asset)
    assert_nil order_filter_type(build(:transaction, bot: bot, status: :submitted, external_status: :cancelled, external_id: 'c1'))
    assert_nil order_filter_type(build(:transaction, bot: bot, status: :submitted, external_status: :abandoned, external_id: 'a1'))
    assert_nil order_filter_type(build(:transaction, bot: bot, status: :failed, external_id: 'f1'))
    assert_nil order_filter_type(build(:transaction, bot: bot, status: :skipped, external_id: 's1'))
  end

  # == inactive_order_row? — used by _order/_order_timeline to dim non-active rows ==

  test 'inactive_order_row? is true for abandoned' do
    assert inactive_order_row?(build(:transaction, status: :submitted, external_status: :abandoned, external_id: 'a1'))
  end

  test 'inactive_order_row? is true for cancelled' do
    assert inactive_order_row?(build(:transaction, status: :submitted, external_status: :cancelled, external_id: 'c1'))
  end

  test 'inactive_order_row? is true for skipped' do
    assert inactive_order_row?(build(:transaction, status: :skipped, external_id: 's1'))
  end

  test 'inactive_order_row? is false for open/closed/unknown rows' do
    bot = create(:dca_single_asset)
    assert_not inactive_order_row?(build(:transaction, bot: bot, status: :submitted, external_status: :open, external_id: 'o1'))
    assert_not inactive_order_row?(build(:transaction, bot: bot, status: :submitted, external_status: :closed, external_id: 'cl1'))
    assert_not inactive_order_row?(build(:transaction, bot: bot, status: :submitted, external_status: :unknown, external_id: 'u1'))
  end

  # == merged trigger dropdown (issues #1/#2) ==

  test 'a buying bot offers [Buy only, Start buying, Start selling] for a timed trigger' do
    bot = create(:dca_single_asset) # buying
    options = trigger_mode_select_options(bot, 'price_limit')

    assert_equal %w[restrict start flip], options.map(&:last)
    assert_equal ['Buy only', 'Start buying', 'Start selling'], options.map(&:first)
  end

  test 'a selling bot offers [Sell only, Start selling, Start buying] for a timed trigger' do
    bot = create(:dca_single_asset)
    bot.direction = 'selling'

    assert_equal ['Sell only', 'Start selling', 'Start buying'],
                 trigger_mode_select_options(bot, 'price_limit').map(&:first)
  end

  test 'price-drop offers only [start, flip] (no restrict — its pause latches)' do
    bot = create(:dca_single_asset)

    assert_equal %w[start flip], trigger_mode_select_options(bot, 'price_drop_limit').map(&:last)
  end

  test 'trigger_mode_for derives the current token from the stored action/timing' do
    bot = create(:dca_single_asset)
    bot.price_limit_timing_condition = 'while'
    bot.price_limit_action = 'pause'
    assert_equal 'restrict', trigger_mode_for(bot, 'price_limit')

    bot.price_limit_timing_condition = 'after'
    assert_equal 'start', trigger_mode_for(bot, 'price_limit')

    bot.price_limit_action = 'start_selling'
    assert_equal 'flip', trigger_mode_for(bot, 'price_limit')
  end

  # == FeeCutter copy inversion (issue #4) ==

  test 'FeeCutter copy says below the price when buying and above when selling' do
    bot = create(:dca_single_asset)
    buy_html = render(partial: 'bots/settings/limit_orders', locals: { bot: bot, method: :patch, path: bot_path(id: bot.id) })
    assert_match(/below the price/i, buy_html)

    bot.direction = 'selling'
    sell_html = render(partial: 'bots/settings/limit_orders', locals: { bot: bot, method: :patch, path: bot_path(id: bot.id) })
    assert_match(/above the price/i, sell_html)
    assert_no_match(/below the price/i, sell_html)
  end

  # == chart_pnl_series (the PnL mode of the bot chart) ==
  #
  # The VALUE curve always climbs for a DCA bot, because the money going in climbs — the shape
  # is deposits rather than performance. PnL makes the invested line the zero line, so the curve
  # is the distance from it, in the quote currency.

  test 'the curve is how far ahead of what was put in' do
    assert_equal [10.0], chart_pnl_series([110.0], [100.0])
  end

  test 'behind is below the line' do
    assert_equal [-20.0], chart_pnl_series([80.0], [100.0])
  end

  # A DCA bot's first points sit on zero because the money just spent has not moved yet.
  test 'a fresh buy sits on the line' do
    assert_equal [0.0, 0.0], chart_pnl_series([100.0, 200.0], [100.0, 200.0])
  end

  # THE reason this is absolute and not a percentage of what has been invested so far. A deposit
  # adds the same amount to both terms, so it must move the curve by nothing — as a ratio it
  # moves the denominator, and a flat market would draw +10% falling to +5%.
  test 'a deposit does not move the curve in a flat market' do
    assert_equal [10.0, 10.0], chart_pnl_series([110.0, 210.0], [100.0, 200.0])
  end

  # Selling turns holdings into proceeds inside the same envelope: value holds its level and
  # invested does not move, so a locked-in gain goes on reading as a gain.
  test 'a locked-in gain stays a gain' do
    assert_equal [20.0, 20.0], chart_pnl_series([120.0, 120.0], [100.0, 100.0])
  end

  test 'the curve is as long as the labels, with nothing dropped' do
    assert_equal [0.0, 5.0], chart_pnl_series([0.0, 105.0], [0.0, 100.0])
  end

  test 'no transactions, no curve' do
    assert_equal [], chart_pnl_series([], [])
  end

  # 0.00 in the same weight as a real balance reads as a number worth comparing, and on a holdings
  # table most of them are not.
  test 'a figure that rounds to nothing is dimmed' do
    assert_equal '<span class="is-zero">0.00</span>', money_figure(0)
    assert_equal '<span class="is-zero">0.00</span>', money_figure(0.004), 'rounds to nothing'
    assert_equal '<span class="is-zero">0.00</span>', money_figure(nil)
  end

  test 'a figure that rounds to something is left alone' do
    assert_equal '1,234.50', money_figure(1234.5)
    assert_equal '0.01', money_figure(0.005), 'rounds up to a real cent'
  end
  # == the dashboard's mini P/L curve ==
  #
  # The zero line is the bottom rule of `.dash-intro`, so y=100 in the viewBox IS that rule and the
  # box above it is the headline's own 7rem. Below the rule is the same box mirrored: a fixed 100
  # units, so nothing about the page's height depends on how bad this account's worst day was.
  #
  # The path is monotone cubic, the interpolation the bot and tracker charts use: a curve that
  # cannot overshoot the points it joins, which matters here because the box is sized to the
  # extremes and an overshoot would be drawn straight into the clip.

  test 'a flat curve rides the zero line' do
    spark = pnl_spark(percent: [0.0, 0.0, 0.0], days: 30)

    assert_equal 'M0,100 C16.67,100 33.33,100 50,100 C66.67,100 83.33,100 100,100', spark[:path]
    assert_equal 50.0, spark[:end_y], 'the line is the middle of the box'
  end

  # The floor exists so a quiet week is not redrawn as a mountain range: reaching the edge of the
  # box has to MEAN something, and what it means is ten percent.
  test 'a swing under ten percent falls short of the edge' do
    spark = pnl_spark(percent: [0.0, 0.02], days: 30)

    assert_equal 'M0,100 C33.33,93.33 66.67,86.67 100,80', spark[:path]
    assert_equal 0.1, spark[:scale]
  end

  test 'ten percent reaches the top edge' do
    assert_equal 'M0,100 C33.33,66.67 66.67,33.33 100,0', pnl_spark(percent: [0.0, 0.10], days: 30)[:path]
  end

  test 'past ten percent the whole curve rescales to the peak' do
    spark = pnl_spark(percent: [0.0, 0.5, 0.25], days: 30)

    assert_equal 0.5, spark[:scale]
    assert spark[:path].start_with?('M0,100 '), 'starts on the zero line'
    assert spark[:path].end_with?(' 100,50'), 'ends halfway up the box'
  end

  # A loss is drawn under the rule, in the half of the box that is always there for it.
  test 'a loss is drawn below the line, on the same scale as a gain' do
    spark = pnl_spark(percent: [0.0, 0.2, -0.1], days: 30)

    assert_equal false, spark[:gain]
    assert spark[:path].end_with?(' 100,150'), 'the last point is half a box under the line'
    assert_equal 75.0, spark[:end_y]
  end

  test 'a curve that only ever falls reaches the bottom edge and none of the top' do
    spark = pnl_spark(percent: [0.0, -0.15], days: 30)

    assert_equal 'M0,100 C33.33,133.33 66.67,166.67 100,200', spark[:path]
    assert_equal 100.0, spark[:end_y], 'the floor of the box'
  end

  # Monotone, not a plain spline: three points that rise and then flatten must not dip on the way,
  # or the curve would draw a loss the account never had.
  test 'a curve that levels off does not dip on the way' do
    spark = pnl_spark(percent: [0.0, 0.1, 0.1], days: 30)
    ys = spark[:path].scan(/[\d.]+,([\d.]+)/).flatten.map(&:to_f)

    assert_equal 0.0, ys.min, 'never rises above the peak it is joining'
    assert_operator ys.max, :<=, 100.0, 'and never falls below where it started'
  end

  # Width is history, not data density: a fortnight-old account gets a short curve rather than a
  # full-width one drawn out of nothing.
  test 'full width takes thirty days of history' do
    assert_equal 50.0, pnl_spark(percent: [0.0, 0.1], days: 15)[:width]
    assert_equal 100.0, pnl_spark(percent: [0.0, 0.1], days: 30)[:width]
    assert_equal 100.0, pnl_spark(percent: [0.0, 0.1], days: 400)[:width]
  end

  test 'the area closes on the zero line at both ends' do
    assert_equal 'M0,100 C33.33,66.67 66.67,33.33 100,0 L100,100 L0,100 Z',
                 pnl_spark(percent: [0.0, 0.1], days: 30)[:area]
  end

  # The dot marking the last point is placed in the WHOLE box, both halves of it, because that is
  # what the plot is drawn in.
  test 'the end dot sits on the last point, in the whole box' do
    assert_equal 0.0, pnl_spark(percent: [0.0, 0.1], days: 30)[:end_y], 'a peak at the very top'
    assert_equal 50.0, pnl_spark(percent: [0.0, -0.1, 0.0], days: 30)[:end_y], 'back on the line, half way down'
  end

  # One figure, two writers: the view here, and the JS that rewrites it under the pointer. They
  # have to agree character for character — a '+' the other one omits is a number that changes
  # shape as it is read.
  test 'the headline labels carry a sign only where one is due' do
    assert_equal '+25.00%', pnl_headline_percent(0.25)
    assert_equal '-10.00%', pnl_headline_percent(-0.1)
    assert_equal '0.00%', pnl_headline_percent(0)
  end

  # The menu icon is a number in a 24px box: it has to stay inside it however many bots there are,
  # and sit on the same optical centre the drawn icons do.
  test 'the bot count icon steps down a size per digit and stays inside the box' do
    [1, 12, 123, 9999].each do |count|
      size = bot_count_font_size(count)
      width = count.to_s.length * size * BotHelper::DIGIT_ADVANCE

      assert_operator width, :<=, 24, "#{count} overflows the icon box"
      assert_operator bot_count_baseline(size), :<, 24
    end
    assert_equal bot_count_font_size(1), bot_count_font_size(99), 'the icon holds its size to two digits'
  end
end
