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

  # ---------- DST: the interval grid must be absolute, not wall-clock ----------

  # An ActiveJob payload carries the Time.zone it was enqueued in, and ActiveJob re-applies that
  # zone (Time.use_zone) around every perform — so a self-rescheduling chain keeps running under
  # whatever zone it was born in. Adding the interval as a CALENDAR duration preserves the local
  # wall clock across a DST change, while the elapsed-interval count above is measured in absolute
  # seconds. A chain anchored in CET and running in CEST therefore computed a checkpoint one hour
  # BEFORE the boundary it had just counted against — a checkpoint in the past, which
  # Bot::ActionJob re-enqueues immediately, at machine speed, for the length of the DST offset.
  # Production hit exactly this: 14,192 ActionJobs in one hour on a single weekly bot, which
  # rate-limited the exchange account for every other bot in that container.
  test 'weekly grid stays on the absolute interval under a DST job zone' do
    anchor = Time.utc(2026, 3, 13, 12, 24, 25)  # CET  (UTC+1)
    travel_to Time.utc(2026, 8, 21, 11, 24, 27) # CEST (UTC+2), just past the wall-clock slot
    bot = create(:dca_single_asset, :started, :weekly)
    bot.started_at = anchor
    bot.save!

    Time.use_zone('Vienna') do
      checkpoint = bot.next_interval_checkpoint_at
      assert_equal anchor + (23 * 1.week.to_i), checkpoint.utc
      assert checkpoint.future?, 'a checkpoint in the past is re-enqueued immediately, forever'
    end
  end

  test 'daily grid stays on the absolute interval under a DST job zone' do
    anchor = Time.utc(2026, 3, 13, 12, 24, 25)
    travel_to Time.utc(2026, 8, 21, 11, 24, 27)
    bot = create(:dca_single_asset, :started)
    bot.started_at = anchor
    bot.save!

    Time.use_zone('Vienna') do
      checkpoint = bot.next_interval_checkpoint_at
      assert_equal Time.utc(2026, 8, 21, 12, 24, 25), checkpoint.utc
      assert checkpoint.future?, 'a checkpoint in the past is re-enqueued immediately, forever'
    end
  end

  # The forward and backward steps must agree. Around the autumn transition a calendar subtraction
  # from an absolute grid point lands 25 hours back, and Bot::Accountable#pending_quote_amount
  # floors an interval count against it — one whole contribution silently dropped.
  test 'previous checkpoint sits exactly one interval before the next under a DST job zone' do
    anchor = Time.utc(2026, 10, 23, 12, 0, 0)  # CEST
    travel_to Time.utc(2026, 10, 25, 11, 0, 0) # the day Vienna falls back to CET
    bot = create(:dca_single_asset, :started)  # daily
    bot.started_at = anchor
    bot.save!

    Time.use_zone('Vienna') do
      assert_equal Time.utc(2026, 10, 25, 12, 0, 0), bot.next_interval_checkpoint_at.utc
      assert_equal Time.utc(2026, 10, 24, 12, 0, 0), bot.last_interval_checkpoint_at.utc
    end
  end

  # Months are calendar objects, not a fixed number of seconds — the month branch stays calendar.
  test 'monthly previous checkpoint keeps calendar semantics' do
    bot = create(:dca_single_asset, :started, :monthly)
    bot.started_at = @now - 1.day
    bot.save!

    assert_equal bot.next_interval_checkpoint_at - 1.month, bot.last_interval_checkpoint_at
  end
end
