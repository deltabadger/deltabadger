require 'test_helper'

class BotHelperTest < ActionView::TestCase
  # Regression: a pending (open, unfilled) limit order has amount_exec == 0.0 (not nil),
  # which broke the old `amount_exec || amount` fallback and rendered "Buying 0.0 X for 0.0 Y".
  # It should now show the requested amounts with open-order wording.
  test 'pending limit buy shows open-order wording with requested (not executed) amounts' do
    order = build(:transaction, side: :buy, status: :submitted, external_status: :open,
                                base: 'CKB', quote: 'USDC',
                                amount: 7107, amount_exec: 0, quote_amount: 10, quote_amount_exec: 0)

    assert_equal 'Open order to buy 7107.0 CKB for 10.0 USDC', transaction_summary(order)
  end

  test 'pending limit sell shows open-order sell wording with requested amounts' do
    order = build(:transaction, side: :sell, status: :submitted, external_status: :open,
                                base: 'CKB', quote: 'USDC',
                                amount: 7107, amount_exec: 0, quote_amount: 10, quote_amount_exec: 0)

    assert_equal 'Open order to sell 7107.0 CKB for 10.0 USDC', transaction_summary(order)
  end

  test 'filled buy shows bought wording with executed amounts' do
    order = build(:transaction, side: :buy, status: :submitted, external_status: :closed,
                                base: 'CKB', quote: 'USDC',
                                amount: 7107, amount_exec: 7107, quote_amount: 10, quote_amount_exec: 9.99)

    assert_equal 'Bought 7107.0 CKB for 9.99 USDC', transaction_summary(order)
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

  test 'order_filter_type maps abandoned to the cancelled tab' do
    assert_equal 'cancelled',
                 order_filter_type(build(:transaction, status: :submitted, external_status: :abandoned, external_id: 'a1'))
  end

  test 'order_filter_type maps cancelled to the cancelled tab' do
    assert_equal 'cancelled',
                 order_filter_type(build(:transaction, status: :submitted, external_status: :cancelled, external_id: 'c1'))
  end

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

  test 'order_filter_type is nil for failed and skipped rows so they stay out of the cancelled tab' do
    # Matches existing view behavior: failed/skipped rows show under "All" / "Other"
    # but must not leak into the cancelled, successful, or waiting filters.
    bot = create(:dca_single_asset)
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
end
