require 'test_helper'

class Bot::StartableTest < ActiveSupport::TestCase
  setup do
    # Tuesday, 2026-05-26 12:00 UTC — fixed pin so weekday/time math is deterministic.
    @now = Time.utc(2026, 5, 26, 12, 0, 0)
    travel_to @now
    @bot = create(:dca_single_asset, status: :stopped)
    # Default bot.user.time_zone is "UTC" (User factory default + db column default),
    # so all the assertions below treat user-local clock time == UTC clock time
    # unless explicitly overridden via update_user_time_zone(...).
  end

  teardown { travel_back }

  # ---------- Settings persistence ----------

  test 'settings round-trip through store_accessor' do
    @bot.start_time_enabled = true
    @bot.start_time_mode = 'monday'
    @bot.start_time_of_day = '09:00'
    @bot.set_missed_quote_amount
    @bot.save!

    reloaded = Bot.find(@bot.id)
    assert_equal true, reloaded.start_time_enabled?
    assert_equal 'monday', reloaded.start_time_mode
    assert_equal '09:00', reloaded.start_time_of_day
  end

  test 'feature defaults to disabled (start_time_enabled? false)' do
    assert_equal false, @bot.start_time_enabled?
  end

  # ---------- parse_params coercion ----------

  test 'parse_params coerces start_time_enabled from "1"/"true"' do
    parsed = @bot.parse_params(start_time_enabled: '1',
                               start_time_mode: 'hour', start_time_of_day: '14:30')
    assert_equal true, parsed[:start_time_enabled]
    assert_equal 'hour', parsed[:start_time_mode]
    assert_equal '14:30', parsed[:start_time_of_day]
  end

  test 'parse_params accepts a flattened weekday mode' do
    parsed = @bot.parse_params(start_time_enabled: '1', start_time_mode: 'wednesday',
                               start_time_of_day: '09:00')
    assert_equal 'wednesday', parsed[:start_time_mode]
    assert_equal '09:00', parsed[:start_time_of_day]
  end

  test 'parse_params returns nil for out-of-range date string instead of raising' do
    # Time#parse raises ArgumentError on out-of-range components like "2026-99-99T00:00".
    # parse_params must absorb this so the PATCH controller flow doesn't 500.
    parsed = nil
    assert_nothing_raised do
      parsed = @bot.parse_params(start_time_enabled: '1', start_time_mode: 'date',
                                 start_at: '2026-99-99T00:00')
    end
    assert_nil parsed[:start_at]
  end

  test 'parse_persisted_start_at returns nil for out-of-range stored value' do
    @bot.start_time_enabled = true
    @bot.start_time_mode = 'date'
    @bot.settings['start_at'] = '2026-99-99T00:00:00Z'

    assert_nothing_raised do
      refute @bot.valid?(:start)
    end
    assert @bot.errors[:start_at].any?
  end

  # ---------- parse_params: date-mode start_at uses bot.user.time_zone ----------

  test 'parse_params parses date-mode start_at in the user time zone' do
    update_user_time_zone('Warsaw') # UTC+2 on 2026-11-11 (CEST)
    parsed = @bot.parse_params(start_time_enabled: '1', start_time_mode: 'date',
                               start_at: '2026-11-11T15:24')
    parsed_time = Time.find_zone!('UTC').parse(parsed[:start_at])
    # 15:24 Warsaw == 13:24 UTC during DST.
    assert_equal Time.utc(2026, 11, 11, 14, 24), parsed_time, # Warsaw is UTC+1 in November (CET, not DST)
                 'wall-clock 15:24 in Warsaw on 2026-11-11 (CET) is 14:24 UTC'
  end

  test 'parse_params parses date-mode start_at as UTC when user time_zone is UTC' do
    parsed = @bot.parse_params(start_time_enabled: '1', start_time_mode: 'date',
                               start_at: '2026-11-11T15:24')
    parsed_time = Time.find_zone!('UTC').parse(parsed[:start_at])
    assert_equal Time.utc(2026, 11, 11, 15, 24), parsed_time
  end

  # ---------- initial_start_at: hour mode (UTC user) ----------

  test 'initial_start_at (hour mode) returns today at HH:MM when still in the future' do
    @bot.start_time_enabled = true
    @bot.start_time_mode = 'hour'
    @bot.start_time_of_day = '14:30'

    # now = 12:00 UTC; today 14:30 is future
    assert_equal Time.utc(2026, 5, 26, 14, 30, 0), @bot.initial_start_at
  end

  test 'initial_start_at (hour mode) rolls to tomorrow when HH:MM has passed today' do
    @bot.start_time_enabled = true
    @bot.start_time_mode = 'hour'
    @bot.start_time_of_day = '09:00'

    # now = 12:00; today 09:00 already passed → tomorrow 09:00
    assert_equal Time.utc(2026, 5, 27, 9, 0, 0), @bot.initial_start_at
  end

  # ---------- initial_start_at: weekday modes (UTC user) ----------

  test 'initial_start_at (monday mode) returns next Monday at HH:MM' do
    @bot.start_time_enabled = true
    @bot.start_time_mode = 'monday'
    @bot.start_time_of_day = '09:00'

    # now = Tuesday 12:00 → next Monday is 2026-06-01 09:00 UTC
    assert_equal Time.utc(2026, 6, 1, 9, 0, 0), @bot.initial_start_at
  end

  test 'initial_start_at (tuesday mode) returns same-day when time is still future' do
    @bot.start_time_enabled = true
    @bot.start_time_mode = 'tuesday'
    @bot.start_time_of_day = '15:00'

    # now = Tuesday 12:00 → Tuesday 15:00 same day
    assert_equal Time.utc(2026, 5, 26, 15, 0, 0), @bot.initial_start_at
  end

  test 'initial_start_at (tuesday mode) rolls a week forward when time has passed' do
    @bot.start_time_enabled = true
    @bot.start_time_mode = 'tuesday'
    @bot.start_time_of_day = '09:00'

    # now = Tuesday 12:00; 09:00 passed → next Tuesday 2026-06-02 09:00
    assert_equal Time.utc(2026, 6, 2, 9, 0, 0), @bot.initial_start_at
  end

  test 'initial_start_at (sunday mode) returns next Sunday' do
    @bot.start_time_enabled = true
    @bot.start_time_mode = 'sunday'
    @bot.start_time_of_day = '12:00'

    # now = Tuesday 2026-05-26 12:00 → next Sunday is 2026-05-31 12:00 UTC
    assert_equal Time.utc(2026, 5, 31, 12, 0, 0), @bot.initial_start_at
  end

  # ---------- initial_start_at: respects user.time_zone ----------

  test 'initial_start_at (hour mode) treats HH:MM as user-local clock time' do
    update_user_time_zone('Warsaw') # CEST (UTC+2) on 2026-05-26
    @bot.start_time_enabled = true
    @bot.start_time_mode = 'hour'
    @bot.start_time_of_day = '14:30' # Warsaw local

    # now = 12:00 UTC = 14:00 Warsaw. 14:30 Warsaw is still future today.
    # 14:30 Warsaw == 12:30 UTC during DST.
    assert_equal Time.utc(2026, 5, 26, 12, 30, 0), @bot.initial_start_at
  end

  test 'initial_start_at (weekday mode) computes weekday in the user time zone' do
    # Use a tz that flips the weekday across midnight. Auckland is UTC+12.
    update_user_time_zone('Auckland')
    @bot.start_time_enabled = true
    @bot.start_time_mode = 'wednesday' # Wednesday in user's tz
    @bot.start_time_of_day = '02:00' # Wed 02:00 Auckland (= Tue 14:00 UTC)

    # now = Tuesday 12:00 UTC = Wednesday 00:00 Auckland.
    # Next Wednesday 02:00 Auckland is today (Tue UTC) at 14:00 UTC.
    assert_equal Time.utc(2026, 5, 26, 14, 0, 0), @bot.initial_start_at
  end

  # ---------- initial_start_at: date mode ----------

  test 'initial_start_at (date mode) returns the stored absolute UTC moment' do
    @bot.start_time_enabled = true
    @bot.start_time_mode = 'date'
    @bot.start_at = '2026-11-11T15:24:00Z'

    assert_equal Time.utc(2026, 11, 11, 15, 24, 0), @bot.initial_start_at
  end

  test 'initial_start_at returns nil when feature disabled' do
    @bot.start_time_enabled = false
    @bot.start_time_mode = 'hour'
    @bot.start_time_of_day = '14:30'

    assert_nil @bot.initial_start_at
  end

  test 'initial_start_at always returns UTC zone' do
    @bot.start_time_enabled = true
    @bot.start_time_mode = 'hour'
    @bot.start_time_of_day = '14:30'

    assert @bot.initial_start_at.utc?
  end

  # ---------- default_start_time_selection: NYSE Monday open (09:30 ET) in user zone ----------

  test 'default_start_time_selection returns Monday and ET-open translated to UTC user' do
    # now = Tuesday 2026-05-26, ET is EDT (UTC-4). NYSE opens 09:30 ET = 13:30 UTC.
    # UTC user sees the same instant as 13:30, still Monday.
    assert_equal %w[monday 13:30], @bot.default_start_time_selection
  end

  test 'default_start_time_selection translates the ET open into the user zone clock' do
    update_user_time_zone('Warsaw') # CEST (UTC+2) in May
    # 09:30 EDT = 13:30 UTC = 15:30 CEST, still Monday.
    assert_equal %w[monday 15:30], @bot.default_start_time_selection
  end

  test 'default_start_time_selection rolls the weekday forward for far-eastern zones' do
    update_user_time_zone('Auckland') # NZST (UTC+12) in May/June — no DST in winter
    # 09:30 EDT Mon = 13:30 UTC Mon = 01:30 Tue in Auckland → Tuesday 01:30.
    assert_equal %w[tuesday 01:30], @bot.default_start_time_selection
  end

  test 'default_start_time_selection honors ET DST (09:30 local year-round)' do
    # Jump to winter: Tuesday 2026-01-06 12:00 UTC. ET is EST (UTC-5).
    # NYSE still opens 09:30 ET, which is 14:30 UTC (vs 13:30 in summer).
    travel_to Time.utc(2026, 1, 6, 12, 0, 0)
    assert_equal %w[monday 14:30], @bot.default_start_time_selection
  ensure
    travel_back
    travel_to @now
  end

  # ---------- repeat_anchor_at: only reads persisted start_at, never recomputes ----------

  test 'repeat_anchor_at returns started_at when feature disabled' do
    @bot.started_at = Time.utc(2026, 5, 26, 12, 0, 0)
    @bot.start_time_enabled = false

    assert_equal Time.utc(2026, 5, 26, 12, 0, 0), @bot.repeat_anchor_at.utc
  end

  test 'repeat_anchor_at returns persisted start_at when feature enabled' do
    @bot.start_time_enabled = true
    @bot.start_time_mode = 'day'
    @bot.settings['start_at'] = '2026-06-01T09:00:00Z'

    assert_equal Time.utc(2026, 6, 1, 9, 0, 0), @bot.repeat_anchor_at.utc
  end

  test 'repeat_anchor_at is stable across travel_to (does NOT recompute)' do
    @bot.start_time_enabled = true
    @bot.start_time_mode = 'hour'
    @bot.start_time_of_day = '09:00'
    @bot.settings['start_at'] = '2026-05-26T09:00:00Z' # frozen baseline (past now)

    first = @bot.repeat_anchor_at
    refute_nil first
    travel 10.days
    second = @bot.repeat_anchor_at
    travel_back

    assert_equal first, second, 'repeat_anchor_at must read persisted value, not recompute'
  end

  # ---------- disable_starting_time! ----------

  test 'disable_starting_time! flips the toggle off and persists' do
    @bot.start_time_enabled = true
    @bot.start_time_mode = 'date'
    @bot.settings['start_at'] = '2026-11-11T15:24:00Z'
    @bot.set_missed_quote_amount
    @bot.save!

    @bot.disable_starting_time!

    reloaded = Bot.find(@bot.id)
    assert_equal false, reloaded.start_time_enabled?,
                 'after disable, the toggle must be off'
  end

  test 'disable_starting_time! is a no-op when feature already off' do
    @bot.start_time_enabled = false
    @bot.set_missed_quote_amount
    @bot.save!

    assert_nothing_raised { @bot.disable_starting_time! }
    assert_equal false, @bot.start_time_enabled?
  end

  test 'repeat_anchor_at falls back to started_at when settings.start_at is blank' do
    @bot.started_at = Time.utc(2026, 5, 26, 12, 0, 0)
    @bot.start_time_enabled = true
    @bot.settings['start_at'] = nil

    assert_equal Time.utc(2026, 5, 26, 12, 0, 0), @bot.repeat_anchor_at.utc
  end

  private

  def update_user_time_zone(zone_name)
    @bot.user.update!(time_zone: zone_name)
    @bot.reload
  end
end
