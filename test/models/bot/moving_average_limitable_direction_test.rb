require 'test_helper'

# M5 — direction-aware MA trigger with a gate (pause) or flip (start selling/buying) action.
class Bot::MovingAverageLimitableDirectionTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

  # A buying bot whose buy-side MA trigger is enabled with the given action, condition stubbed.
  def buy_trigger_bot(action: 'pause', met: true)
    bot = create(:dca_single_asset, :started)
    bot.moving_average_limited = true
    bot.moving_average_limit_action = action
    bot.set_missed_quote_amount
    bot.save!
    bot.stubs(:get_moving_average_limit_condition_met?).returns(Result::Success.new(met))
    bot.stubs(:funds_are_low?).returns(false)
    bot
  end

  # == Active config selection ==

  test 'active config follows direction' do
    bot = create(:dca_single_asset, :started)
    bot.moving_average_limited = true
    bot.moving_average_limit_action = 'start_selling'
    bot.sell_moving_average_limited = false
    bot.sell_moving_average_limit_action = 'start_buying'
    bot.set_missed_quote_amount
    bot.save!

    assert_predicate bot, :active_moving_average_limited?
    assert_equal 'start_selling', bot.active_moving_average_limit_action

    bot.direction = 'selling'
    assert_not_predicate bot, :active_moving_average_limited? # sell side disabled
    assert_equal 'start_buying', bot.active_moving_average_limit_action
  end

  test 'the action defaults to pause and is never persisted on load' do
    bot = create(:dca_single_asset)
    assert_equal 'pause', bot.moving_average_limit_action
    assert_equal 'pause', bot.sell_moving_average_limit_action
    assert_not bot.settings.key?('moving_average_limit_action')
    assert_not bot.settings.key?('sell_moving_average_limit_action')
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
    bot.sell_moving_average_limited = true
    bot.sell_moving_average_limit_action = 'start_buying'
    bot.direction = 'selling'
    bot.set_missed_quote_amount
    bot.save!
    bot.stubs(:get_moving_average_limit_condition_met?).returns(Result::Success.new(true))
    bot.stubs(:funds_are_low?).returns(false)
    bot.expects(:set_order).never

    bot.execute_action

    assert_predicate bot.reload, :buying?
  end

  # == Per-side condition timestamp isolation ==

  test 'get_moving_average_limit_condition_met? writes the active sides own condition_met_at' do
    bot = create(:dca_single_asset, :started)
    bot.sell_moving_average_limited = true
    bot.sell_moving_average_limit_value_condition = 'above'
    bot.sell_moving_average_limit_in_ticker_id = bot.ticker.id
    bot.sell_moving_average_limit_in_ma_type = 'sma'
    bot.sell_moving_average_limit_in_timeframe = 'one_day'
    bot.sell_moving_average_limit_in_period = 9
    bot.direction = 'selling'
    bot.set_missed_quote_amount
    bot.save!
    Ticker.any_instance.stubs(:get_last_price).returns(Result::Success.new(150.to_d))
    Ticker.any_instance.stubs(:get_sma_value).returns(Result::Success.new(100.to_d)) # price > MA → met

    result = bot.get_moving_average_limit_condition_met?

    assert result.data
    assert bot.reload.sell_moving_average_limit_condition_met_at.present?,
           'writes the SELL met_at while selling'
    assert_nil bot.moving_average_limit_condition_met_at, 'leaves the BUY met_at untouched'
  end

  test 'a flip clears both sides condition_met_at so an after-timing timestamp cannot leak' do
    bot = create(:dca_single_asset, :started)
    bot.set_missed_quote_amount
    bot.save!
    bot.update_columns(transient_data: bot.transient_data.merge(
      'moving_average_limit_condition_met_at' => 1.hour.ago.iso8601,
      'sell_moving_average_limit_condition_met_at' => 1.hour.ago.iso8601
    ))

    bot.flip_direction!

    assert_nil bot.reload.moving_average_limit_condition_met_at
    assert_nil bot.sell_moving_average_limit_condition_met_at
  end
end
