require 'test_helper'

# Tests for Automation::Schedulable#next_interval_checkpoint_at after the
# Bot::Startable integration. The .future? guard and the repeat_anchor_at
# baseline behavior live here.
class Automation::SchedulableTest < ActiveSupport::TestCase
  setup do
    # Pin clock to Tuesday 2026-05-26 12:00 UTC.
    @now = Time.utc(2026, 5, 26, 12, 0, 0)
    travel_to @now
  end

  teardown { travel_back }

  # ---------- Regression: feature disabled keeps today's behavior ----------

  test 'feature disabled: daily interval advances from started_at as today' do
    bot = create(:dca_single_asset, :started)
    bot.started_at = @now - 30.minutes
    bot.save!

    # Existing math: ((now - checkpoint) / interval).ceil intervals from started_at
    # 30 minutes < 1 day → 1 interval ahead → started_at + 1 day
    assert_equal bot.started_at + 1.day, bot.next_interval_checkpoint_at
  end

  test 'feature disabled: monthly interval advances by 1.month from started_at' do
    bot = create(:dca_single_asset, :started, :monthly)
    bot.started_at = @now - 1.day
    bot.save!

    expected = bot.started_at + 1.month
    assert_equal expected, bot.next_interval_checkpoint_at
  end

  # ---------- .future? guard: future anchor returned as-is ----------

  test 'future start_at + monthly interval returns exactly start_at (month loop does not skip first run)' do
    bot = create(:dca_single_asset, :started, :monthly)
    future_anchor = Time.utc(2026, 6, 15, 9, 0, 0) # > now
    bot.start_time_enabled = true
    bot.start_time_mode = 'date'
    bot.settings['start_at'] = future_anchor.iso8601
    bot.set_missed_quote_amount
    bot.save!

    assert_equal future_anchor, bot.next_interval_checkpoint_at.utc,
                 'future anchor must be returned as-is for monthly interval'
  end

  test 'future start_at + hourly interval returns exactly start_at' do
    bot = create(:dca_single_asset, :started, :hourly)
    future_anchor = @now + 3.hours
    bot.start_time_enabled = true
    bot.start_time_mode = 'hour'
    bot.start_time_of_day = future_anchor.strftime('%H:%M')
    bot.settings['start_at'] = future_anchor.iso8601
    bot.set_missed_quote_amount
    bot.save!

    assert_equal future_anchor, bot.next_interval_checkpoint_at.utc
  end

  # ---------- Past anchor: advances from anchor ----------

  test 'past start_at: daily interval advances by interval from start_at' do
    bot = create(:dca_single_asset, :started)
    past_anchor = Time.utc(2026, 5, 24, 16, 0, 0) # 2 days + 4 hours ago
    bot.start_time_enabled = true
    bot.start_time_mode = 'date'
    bot.settings['start_at'] = past_anchor.iso8601
    bot.set_missed_quote_amount
    bot.save!

    # next 16:00 UTC after now (Tuesday 12:00) is today 16:00
    assert_equal Time.utc(2026, 5, 26, 16, 0, 0), bot.next_interval_checkpoint_at.utc
  end

  test 'past start_at: weekly interval lands on next Monday 09:00' do
    bot = create(:dca_single_asset, :started, :weekly)
    past_anchor = Time.utc(2026, 5, 18, 9, 0, 0) # Monday 2 weeks ago
    bot.start_time_enabled = true
    bot.start_time_mode = 'monday'
    bot.start_time_of_day = '09:00'
    bot.settings['start_at'] = past_anchor.iso8601
    bot.set_missed_quote_amount
    bot.save!

    # next Monday after now is 2026-06-01 09:00 UTC
    assert_equal Time.utc(2026, 6, 1, 9, 0, 0), bot.next_interval_checkpoint_at.utc
  end

  # ---------- Default behavior for non-Startable schedulable models ----------

  test 'repeat_anchor_at default falls back to started_at when feature disabled' do
    bot = create(:dca_single_asset, :started)
    bot.started_at = @now - 5.minutes
    bot.save!

    # With feature disabled, repeat_anchor_at must equal started_at
    assert_equal bot.started_at, bot.repeat_anchor_at
  end
end
