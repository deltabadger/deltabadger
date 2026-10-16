require 'test_helper'

# Each condition rule latches on its OWN state. The price-drop, moving-average and indicator rules
# once shared one method name, so the last-included module's latch answered for all three.
class Bot::ConditionTimingLatchTest < ActiveSupport::TestCase
  setup do
    @bot = create(:dca_single_asset, :started)
    # Live evaluation always says "not met": only a latch can make a rule pass.
    Ticker.any_instance.stubs(:get_last_price).returns(Result::Success.new(100))
    @bot.stubs(:price_drop_reference_price_result).returns(Result::Success.new(nil))
    @bot.stubs(:get_moving_average_value).returns(Result::Success.new(100))
    @bot.stubs(:moving_average_condition_satisfied?).returns(false)
    @bot.stubs(:get_indicator_value).returns(Result::Success.new(50))
    @bot.stubs(:indicator_condition_satisfied?).returns(false)
    @bot.stubs(:broadcast_price_drop_limit_info_update)
    @bot.stubs(:broadcast_moving_average_limit_info_update)
    @bot.stubs(:broadcast_indicator_limit_info_update)
  end

  # Two saves: turning a rule on clears its condition_met_at, so the latch is written after.
  def configure(attrs)
    met_at, rule = attrs.partition { |key, _| key.end_with?('_condition_met_at') }.map(&:to_h)
    [rule, met_at].each do |part|
      @bot.assign_attributes(part)
      @bot.set_missed_quote_amount
      @bot.save!
    end
  end

  def met_indicator_after!
    configure(indicator_limited: true, indicator_limit_timing_condition: 'after',
              indicator_limit_condition_met_at: 1.hour.ago.iso8601)
  end

  test 'a met after-timing indicator rule does not make the price-drop rule pass' do
    met_indicator_after!
    configure(price_drop_limited: true, price_drop_limit_condition_met_at: nil)

    assert_equal false, @bot.get_price_drop_limit_condition_met?.data
  end

  test 'a met after-timing indicator rule does not make the moving-average rule pass' do
    met_indicator_after!
    configure(moving_average_limited: true, moving_average_limit_timing_condition: 'after',
              moving_average_limit_condition_met_at: nil)

    assert_equal false, @bot.get_moving_average_limit_condition_met?.data
  end

  test 'the moving-average after timing latches on its own state' do
    configure(moving_average_limited: true, moving_average_limit_timing_condition: 'after',
              moving_average_limit_condition_met_at: 1.hour.ago.iso8601)

    assert_equal true, @bot.get_moving_average_limit_condition_met?.data
  end

  test 'the moving-average while timing does not latch' do
    configure(moving_average_limited: true, moving_average_limit_timing_condition: 'while',
              moving_average_limit_condition_met_at: 1.hour.ago.iso8601)

    assert_equal false, @bot.get_moving_average_limit_condition_met?.data
  end

  test 'the price-drop rule latches on its own state' do
    configure(price_drop_limited: true, price_drop_limit_condition_met_at: 1.hour.ago.iso8601)

    assert_equal true, @bot.get_price_drop_limit_condition_met?.data
  end

  test 'the indicator after timing latches on its own state' do
    met_indicator_after!

    assert_equal true, @bot.get_indicator_limit_condition_met?.data
  end
end
