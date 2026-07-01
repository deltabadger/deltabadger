require 'test_helper'

# M2b — Scheduling per direction. The active side's cadence drives the schedule, the buy-only
# smart-interval math is bypassed while selling, and the sell cadence is unchangeable while it
# is the active side (parity with the buy interval).
class Bot::ReversibleSchedulingTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

  # The sell cadence is configured while BUYING (the sell side is inactive then), then the bot
  # is flipped to selling — mirroring the real UX and the unchangeable-while-active constraint.
  def sell_bot(sell_interval: nil, smart: false)
    bot = create(:dca_single_asset, :started) # buy interval 'day'
    bot.sell_interval = sell_interval if sell_interval
    if smart
      bot.smart_intervaled = true
      bot.smart_interval_quote_amount = 10
    end
    bot.set_missed_quote_amount
    bot.save! # save sell config while buying (allowed)

    bot.direction = 'selling'
    bot.set_missed_quote_amount
    bot.save! # flip to selling; sell_interval unchanged → allowed
    bot
  end

  # == effective_interval_duration is direction-aware ==

  test 'effective_interval_duration uses the sell cadence while selling' do
    assert_equal 1.week, sell_bot(sell_interval: 'week').effective_interval_duration
  end

  test 'effective_interval_duration falls back to the buy interval when sell_interval is blank' do
    assert_equal 1.day, sell_bot.effective_interval_duration
  end

  test 'effective_interval_duration uses the buy cadence while buying (sell config ignored)' do
    bot = create(:dca_single_asset, :started)
    bot.sell_interval = 'week'
    bot.set_missed_quote_amount
    bot.save!
    assert_predicate bot, :buying?
    assert_equal 1.day, bot.effective_interval_duration
  end

  test 'a selling bot bypasses smart-interval math and uses the sell cadence' do
    # While buying, smart interval would shrink day → day/10. While selling, it must be 1.week.
    assert_equal 1.week, sell_bot(sell_interval: 'week', smart: true).effective_interval_duration
  end

  test 'a buying smart-intervaled bot still shrinks the cadence (unchanged)' do
    bot = create(:dca_single_asset, :started)
    bot.smart_intervaled = true
    bot.smart_interval_quote_amount = 10
    bot.set_missed_quote_amount
    bot.save!
    # day / (quote_amount 100 / smart 10) = day / 10
    assert_in_delta (1.day / 10).to_f, bot.effective_interval_duration.to_f, 1.0
  end

  # == restarting_within_interval? does not use the (frozen) buy carry while selling ==

  test 'a selling bot just inside its cadence is restarting_within_interval?' do
    bot = sell_bot # sell_interval falls back to 'day'
    bot.status = :stopped
    bot.last_action_job_at = 1.minute.ago.iso8601
    assert_predicate bot, :restarting?
    assert_predicate bot, :restarting_within_interval?
  end

  test 'a selling bot past its cadence is not restarting_within_interval?' do
    bot = sell_bot
    bot.status = :stopped
    bot.last_action_job_at = 2.days.ago.iso8601
    assert_not_predicate bot, :restarting_within_interval?
  end

  # == sell_interval unchangeable while it is the active side ==

  test 'sell_interval cannot be changed while actively selling' do
    bot = sell_bot(sell_interval: 'week')
    bot.sell_interval = 'month'
    bot.set_missed_quote_amount

    assert_not bot.valid?(:update)
    assert bot.errors[:settings].present?
  end

  test 'sell_interval can be changed while buying (sell side is inactive)' do
    bot = create(:dca_single_asset, :started) # buying
    bot.sell_interval = 'week'
    bot.set_missed_quote_amount

    assert bot.valid?(:update)
  end
end
