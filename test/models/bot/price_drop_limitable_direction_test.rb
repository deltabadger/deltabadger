require 'test_helper'

# M5 — direction-aware price-drop trigger with a gate (pause) or flip (start selling/buying) action.
class Bot::PriceDropLimitableDirectionTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

  # A buying bot whose buy-side price-drop trigger is enabled with the given action, condition stubbed.
  def buy_trigger_bot(action: 'pause', met: true)
    bot = create(:dca_single_asset, :started)
    bot.price_drop_limited = true
    bot.price_drop_limit_action = action
    bot.set_missed_quote_amount
    bot.save!
    bot.stubs(:get_price_drop_limit_condition_met?).returns(Result::Success.new(met))
    bot.stubs(:funds_are_low?).returns(false)
    bot
  end

  # == Active config selection ==

  test 'active config follows direction' do
    bot = create(:dca_single_asset, :started)
    bot.price_drop_limited = true
    bot.price_drop_limit_action = 'start_selling'
    bot.sell_price_drop_limited = false
    bot.sell_price_drop_limit_action = 'start_buying'
    bot.set_missed_quote_amount
    bot.save!

    assert_predicate bot, :active_price_drop_limited?
    assert_equal 'start_selling', bot.active_price_drop_limit_action

    bot.direction = 'selling'
    assert_not_predicate bot, :active_price_drop_limited? # sell side disabled
    assert_equal 'start_buying', bot.active_price_drop_limit_action
  end

  test 'the action defaults to pause and is never persisted on load' do
    bot = create(:dca_single_asset)
    assert_equal 'pause', bot.price_drop_limit_action
    assert_equal 'pause', bot.sell_price_drop_limit_action
    assert_not bot.settings.key?('price_drop_limit_action')
    assert_not bot.settings.key?('sell_price_drop_limit_action')
  end

  # == Gate (pause) action — today's behaviour, unchanged ==

  test 'a pause trigger that is met trades normally' do
    bot = buy_trigger_bot(action: 'pause', met: true)
    bot.expects(:set_order).returns(Result::Success.new)

    bot.execute_action
  end

  test 'a pause trigger that is not met pauses and does not trade' do
    bot = buy_trigger_bot(action: 'pause', met: false)
    bot.expects(:set_order).never

    result = bot.execute_action

    assert result.data[:break_reschedule]
    assert_equal 'waiting', bot.reload.status
  end

  # == Flip action — the new trading-bot behaviour ==

  test 'a met flip trigger flips direction and places no order on the flipping tick' do
    bot = buy_trigger_bot(action: 'start_selling', met: true)
    bot.expects(:set_order).never
    assert_predicate bot, :buying?

    result = bot.execute_action

    assert result.data[:break_reschedule]
    assert_predicate bot.reload, :selling?
  end

  test 'an unmet flip trigger does NOT pause — it trades normally (only watches)' do
    bot = buy_trigger_bot(action: 'start_selling', met: false)
    bot.expects(:set_order).returns(Result::Success.new)
    assert_predicate bot, :buying?

    bot.execute_action

    assert_predicate bot.reload, :buying? # no flip, no pause
  end

  test 'a sell-side flip trigger flips a selling bot back to buying' do
    bot = create(:dca_single_asset, :started)
    bot.sell_price_drop_limited = true
    bot.sell_price_drop_limit_action = 'start_buying'
    bot.direction = 'selling'
    bot.set_missed_quote_amount
    bot.save!
    bot.stubs(:get_price_drop_limit_condition_met?).returns(Result::Success.new(true))
    bot.stubs(:funds_are_low?).returns(false)
    bot.expects(:set_order).never

    bot.execute_action

    assert_predicate bot.reload, :buying?
  end

  # == Per-side condition timestamp isolation ==

  # == sell-side inversion: "rise from a recent low", not "drop from high" (issue: price-drop spirit) ==

  test 'the sell condition triggers on a RISE from the recent low and reads get_low_of_last' do
    bot = create(:dca_single_asset, :started)
    bot.sell_price_drop_limited = true
    bot.sell_price_drop_limit = 0.2
    bot.sell_price_drop_limit_time_window_condition = 'twenty_four_hours'
    bot.sell_price_drop_limit_in_ticker_id = bot.ticker.id
    bot.direction = 'selling'
    bot.set_missed_quote_amount
    bot.save!
    # current price (130) > (1 + 0.2) * 100 = 120 → rise condition met; uses the LOW, not the high
    Ticker.any_instance.stubs(:get_last_price).returns(Result::Success.new(130.to_d))
    Ticker.any_instance.stubs(:get_low_of_last).returns(Result::Success.new(100.to_d))
    Ticker.any_instance.expects(:get_high_of_last).never

    result = bot.get_price_drop_limit_condition_met?

    assert result.data, 'a 30% rise from the 100 low clears the 20% threshold'
    assert bot.reload.sell_price_drop_limit_condition_met_at.present?, 'writes the SELL met_at while selling'
    assert_nil bot.price_drop_limit_condition_met_at, 'leaves the BUY met_at untouched'
  end

  test 'the sell condition is NOT met when price has not risen enough above the low' do
    bot = create(:dca_single_asset, :started)
    bot.sell_price_drop_limited = true
    bot.sell_price_drop_limit = 0.2
    bot.sell_price_drop_limit_time_window_condition = 'twenty_four_hours'
    bot.sell_price_drop_limit_in_ticker_id = bot.ticker.id
    bot.direction = 'selling'
    bot.set_missed_quote_amount
    bot.save!
    # current price (110) < (1 + 0.2) * 100 = 120 → only a 10% rise, threshold not cleared
    Ticker.any_instance.stubs(:get_last_price).returns(Result::Success.new(110.to_d))
    Ticker.any_instance.stubs(:get_low_of_last).returns(Result::Success.new(100.to_d))

    assert_not bot.get_price_drop_limit_condition_met?.data
  end

  test 'the buy condition still triggers on a DROP from the high' do
    bot = create(:dca_single_asset, :started)
    bot.price_drop_limited = true
    bot.price_drop_limit = 0.2
    bot.price_drop_limit_time_window_condition = 'twenty_four_hours'
    bot.price_drop_limit_in_ticker_id = bot.ticker.id
    bot.set_missed_quote_amount
    bot.save!
    # current price (50_000) < (1 - 0.2) * 100_000 = 80_000 → drop condition met
    Ticker.any_instance.stubs(:get_last_price).returns(Result::Success.new(50_000.to_d))
    Ticker.any_instance.stubs(:get_high_of_last).returns(Result::Success.new(100_000.to_d))
    Ticker.any_instance.expects(:get_low_of_last).never

    assert bot.get_price_drop_limit_condition_met?.data
  end

  # == window sets per direction: ATL is dropped on the sell side; default is 24h ==

  test 'the sell-side time-window options exclude all-time-low and default to 24h' do
    bot = create(:dca_single_asset)
    bot.direction = 'selling'

    sell_keys = Bot::PriceDropLimitable::PRICE_DROP_LIMIT_SELL_TIME_WINDOW_CONDITIONS.keys
    assert_not_includes sell_keys, 'ath'
    assert_includes sell_keys, 'twenty_four_hours'
    assert_equal 'twenty_four_hours', bot.sell_price_drop_limit_time_window_condition
  end

  test 'a legacy persisted ath on the sell field reads back as twenty_four_hours' do
    bot = create(:dca_single_asset)
    bot.update_columns(settings: bot.settings.merge('sell_price_drop_limit_time_window_condition' => 'ath'))

    assert_equal 'twenty_four_hours', bot.reload.sell_price_drop_limit_time_window_condition
  end

  test 'a flip clears both sides condition_met_at so a stale timestamp cannot leak' do
    bot = create(:dca_single_asset, :started)
    bot.set_missed_quote_amount
    bot.save!
    bot.update_columns(transient_data: bot.transient_data.merge(
      'price_drop_limit_condition_met_at' => 1.hour.ago.iso8601,
      'sell_price_drop_limit_condition_met_at' => 1.hour.ago.iso8601
    ))

    bot.flip_direction!

    assert_nil bot.reload.price_drop_limit_condition_met_at
    assert_nil bot.sell_price_drop_limit_condition_met_at
  end
end
