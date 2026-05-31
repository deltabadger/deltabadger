require 'test_helper'

class Bots::StartingTimeTest < ActionDispatch::IntegrationTest
  setup do
    # Sunday 2026-05-24 12:34 UTC — used for the hourly-cadence regression.
    @sunday = Time.utc(2026, 5, 24, 12, 34, 0)
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    SolidQueue::Job.destroy_all
    SolidQueue::ScheduledExecution.destroy_all
  end

  # ---------- UI: rule rendering and persistence via BotsController#update ----------

  test 'shows starting time rule on the bot settings page' do
    bot = create(:dca_single_asset, user: @user, status: :stopped)
    get bot_path(id: bot.id)
    assert_response :ok
    assert_select "form.widget.rule input[type=checkbox][name='bots_dca_single_asset[start_time_enabled]']"
    assert_select "form.widget.rule select[name='bots_dca_single_asset[start_time_mode]']"
  end

  test 'starting time rule defaults to the NYSE Monday open translated to user zone' do
    travel_to Time.utc(2026, 5, 26, 12, 0, 0) do # Tuesday, ET is EDT
      @user.update!(time_zone: 'Warsaw')
      bot = create(:dca_single_asset, user: @user, status: :stopped)
      get bot_path(id: bot.id)
      assert_response :ok

      # 09:30 EDT = 15:30 CEST, Monday — pre-selected before the user touches anything.
      assert_select(
        "form.widget.rule select[name='bots_dca_single_asset[start_time_mode]'] " \
        'option[value=monday][selected=selected]'
      )
      assert_select "form.widget.rule input[type=time][name='bots_dca_single_asset[start_time_of_day]'][value='15:30']"
    end
  end

  test 'submitting date-mode settings persists start_at as UTC ISO8601' do
    bot = create(:dca_single_asset, user: @user, status: :stopped)
    patch bot_path(id: bot.id), params: {
      bots_dca_single_asset: {
        start_time_enabled: '1',
        start_time_mode: 'date',
        start_at: '2026-11-11T15:24'
      }
    }
    bot.reload
    assert_equal true, bot.start_time_enabled?
    assert_equal 'date', bot.start_time_mode
    # Default user.time_zone is UTC → wall clock equals UTC clock.
    assert_equal Time.utc(2026, 11, 11, 15, 24), Time.find_zone!('UTC').parse(bot.start_at)
  end

  test 'submitted datetime is interpreted in user time_zone' do
    @user.update!(time_zone: 'Warsaw')
    bot = create(:dca_single_asset, user: @user, status: :stopped)
    patch bot_path(id: bot.id), params: {
      bots_dca_single_asset: {
        start_time_enabled: '1',
        start_time_mode: 'date',
        start_at: '2026-11-11T15:24' # 15:24 Warsaw (CET, UTC+1)
      }
    }
    bot.reload
    parsed = Time.find_zone!('UTC').parse(bot.start_at)
    assert_equal Time.utc(2026, 11, 11, 14, 24), parsed,
                 '15:24 in Warsaw (CET) is 14:24 UTC'
  end

  # ---------- PATCH start_at overwrites stale value (no silent preservation) ----------

  test 'PATCH with blank start_at clears the previously stored value' do
    bot = create(:dca_single_asset, user: @user, status: :stopped)
    bot.settings['start_at'] = '2026-06-01T09:00:00Z'
    bot.set_missed_quote_amount
    bot.save!

    patch bot_path(id: bot.id), params: {
      bots_dca_single_asset: {
        start_time_enabled: '1',
        start_time_mode: 'date',
        start_at: '' # explicit clear
      }
    }

    bot.reload
    assert_nil bot.settings['start_at'],
               'blank start_at submission must overwrite, not preserve, the stored value'
  end

  test 'PATCH with malformed start_at clears the previously stored value' do
    bot = create(:dca_single_asset, user: @user, status: :stopped)
    bot.settings['start_at'] = '2026-06-01T09:00:00Z'
    bot.set_missed_quote_amount
    bot.save!

    patch bot_path(id: bot.id), params: {
      bots_dca_single_asset: {
        start_time_enabled: '1',
        start_time_mode: 'date',
        start_at: '2026-99-99T00:00' # out of range → parse returns nil
      }
    }

    bot.reload
    assert_nil bot.settings['start_at'],
               'malformed start_at must overwrite, not silently preserve, the stored value'
  end

  # ---------- Start: future anchor schedules wait_until, no immediate ActionJob ----------

  test 'starting with future anchor schedules wait_until at start_at, not now' do
    travel_to @sunday do
      bot = create(:dca_single_asset, :hourly, user: @user, status: :stopped)
      bot.start_time_enabled = true
      bot.start_time_mode = 'monday'
      bot.start_time_of_day = '09:00'
      bot.set_missed_quote_amount
      bot.save!

      expected_first_run = Time.utc(2026, 5, 25, 9, 0, 0) # Monday 09:00 UTC

      assert bot.start(start_fresh: true)
      bot.reload

      assert_equal expected_first_run, bot.started_at.utc
      assert_equal expected_first_run.iso8601,
                   Time.find_zone!('UTC').parse(bot.settings['start_at']).iso8601

      # Assert the scheduled execution time itself — catches accidental immediate enqueue.
      scheduled = SolidQueue::ScheduledExecution.joins(:job)
                                                .where(solid_queue_jobs: { class_name: 'Bot::ActionJob' })
                                                .order(:scheduled_at)
                                                .last
      refute_nil scheduled, 'expected a scheduled Bot::ActionJob'
      assert_in_delta expected_first_run.to_f, scheduled.scheduled_at.to_f, 1.0
    end
  end

  test 'starting with feature disabled keeps current immediate-fire behavior' do
    bot = create(:dca_single_asset, user: @user, status: :stopped)
    bot.start_time_enabled = false
    bot.set_missed_quote_amount
    bot.save!

    assert bot.start(start_fresh: true)
    # Immediate enqueue → no future scheduled execution.
    scheduled_count = SolidQueue::ScheduledExecution.joins(:job)
                                                    .where(solid_queue_jobs: { class_name: 'Bot::ActionJob' })
                                                    .where('solid_queue_scheduled_executions.scheduled_at > ?', 1.minute.from_now)
                                                    .count
    assert_equal 0, scheduled_count, 'no future-scheduled job expected when feature disabled'
  end

  # ---------- Validation ----------

  test 'hour mode with malformed start_time_of_day fails start validation, no exception' do
    bot = create(:dca_single_asset, user: @user, status: :stopped)
    bot.start_time_enabled = true
    bot.start_time_mode = 'hour'
    bot.start_time_of_day = '99:99'
    bot.set_missed_quote_amount
    bot.save!

    refute bot.start(start_fresh: true)
    assert bot.errors[:start_time_of_day].any?, 'expected validation error on :start_time_of_day'
  end

  test 'unknown start_time_mode fails start validation' do
    bot = create(:dca_single_asset, user: @user, status: :stopped)
    bot.start_time_enabled = true
    bot.start_time_mode = 'wat'
    bot.set_missed_quote_amount
    bot.save!

    refute bot.start(start_fresh: true)
    assert bot.errors[:start_time_mode].any?
  end

  test 'date mode with past datetime fails start validation' do
    bot = create(:dca_single_asset, user: @user, status: :stopped)
    bot.start_time_enabled = true
    bot.start_time_mode = 'date'
    bot.settings['start_at'] = '2020-01-01T00:00:00Z'
    bot.set_missed_quote_amount
    bot.save!

    refute bot.start(start_fresh: true)
    assert bot.errors[:start_at].any?, 'expected validation error on :start_at'
  end

  # ---------- start_fresh: false preserves schedule ----------

  test 'restart (start_fresh: false) does not recompute or shift the schedule' do
    # Seed an OLD start_at deliberately; advance the clock; assert it stays byte-for-byte.
    seeded_start_at_string = '2024-01-15T09:00:00Z'
    seeded_started_at = Time.utc(2024, 1, 15, 9, 0, 0)

    bot = create(:dca_single_asset, :hourly, user: @user)
    bot.assign_attributes(
      status: :stopped,
      started_at: seeded_started_at,
      settings: bot.settings.merge(
        'start_time_enabled' => true,
        'start_time_mode' => 'monday',
        'start_time_of_day' => '09:00',
        'start_at' => seeded_start_at_string
      )
    )
    bot.set_missed_quote_amount
    bot.save!

    # Travel well past the seeded values so any accidental recompute would visibly change things.
    travel_to @sunday do
      assert bot.start(start_fresh: false)
      bot.reload

      assert_equal seeded_started_at, bot.started_at,
                   'restart must not shift started_at'
      # Byte-for-byte: catches accidental re-parse/normalize cycles.
      assert_equal seeded_start_at_string, bot.settings['start_at'],
                   'restart must leave persisted start_at string byte-for-byte unchanged'
    end
  end

  # ---------- DCA Index coverage (lightweight: feature is wired up, not full behavior matrix) ----------

  test 'DCA Index bot settings page renders the starting time rule' do
    bot = create(:dca_index, user: @user, status: :stopped)
    create(:api_key, user: @user, exchange: bot.exchange, key_type: :trading)
    get bot_path(id: bot.id)
    assert_response :ok
    assert_select "form.widget.rule input[type=checkbox][name='bots_dca_index[start_time_enabled]']"
    assert_select "form.widget.rule select[name='bots_dca_index[start_time_mode]']"
  end

  test 'DCA Index permits and persists the new settings keys' do
    bot = create(:dca_index, user: @user, status: :stopped)
    create(:api_key, user: @user, exchange: bot.exchange, key_type: :trading)
    patch bot_path(id: bot.id), params: {
      bots_dca_index: {
        start_time_enabled: '1',
        start_time_mode: 'date',
        start_at: '2026-11-11T15:24'
      }
    }
    bot.reload
    assert_equal true, bot.start_time_enabled?
    assert_equal 'date', bot.start_time_mode
    assert_equal Time.utc(2026, 11, 11, 15, 24), Time.find_zone!('UTC').parse(bot.start_at)
  end

  # ---------- After first execution, the feature auto-disables ----------

  test 'first ActionJob run flips start_time_enabled off' do
    bot = create(:dca_single_asset, :hourly, user: @user)
    bot.assign_attributes(
      status: :scheduled,
      started_at: Time.utc(2026, 5, 25, 9, 0, 0),
      settings: bot.settings.merge(
        'start_time_enabled' => true,
        'start_time_mode' => 'monday',
        'start_time_of_day' => '09:00',
        'start_at' => '2026-05-25T09:00:00Z'
      )
    )
    bot.set_missed_quote_amount
    bot.save!
    setup_bot_execution_mocks(bot)
    bot.stubs(:broadcast_below_minimums_warning)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    assert bot.start_time_enabled?, 'precondition: feature is enabled before the first run'

    Bot::ActionJob.perform_now(bot)

    assert_equal false, bot.reload.start_time_enabled?,
                 'first action run must flip the starting-time feature off'
    # Persisted start_at is left as a historical record of what was scheduled.
    assert_equal '2026-05-25T09:00:00Z', bot.settings['start_at']
  end

  test 'hourly bot, Sunday click + Monday 09:00 start: next run after first is Monday 10:00' do
    travel_to @sunday do
      bot = create(:dca_single_asset, :hourly, user: @user, status: :stopped)
      bot.start_time_enabled = true
      bot.start_time_mode = 'monday'
      bot.start_time_of_day = '09:00'
      bot.set_missed_quote_amount
      bot.save!

      assert bot.start(start_fresh: true)
      bot.reload

      monday_nine = Time.utc(2026, 5, 25, 9, 0, 0)
      assert_equal monday_nine, bot.started_at.utc,
                   'started_at must equal initial_start_at, not Sunday click time'
    end

    # Simulate the first ActionJob having run at Monday 09:00.
    travel_to Time.utc(2026, 5, 25, 9, 0, 1) do
      bot = Bots::DcaSingleAsset.last
      # next checkpoint after first run: started_at + 1 hour = Monday 10:00 UTC
      assert_equal Time.utc(2026, 5, 25, 10, 0, 0), bot.next_interval_checkpoint_at.utc,
                   'next run must be Monday 10:00, not anchored to Sunday 12:34'
    end
  end
end
