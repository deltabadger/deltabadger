require 'test_helper'
require 'fugit'

# Guards the cross-repo ordering that bit us once: hosted containers pull the deltabadger-sourced
# Nasdaq index in `sync_indices_from_coingecko_job`. data-api publishes that index when its 10:00 UTC
# fundamentals run has captured the members' caps (a few minutes in), and the container syncs the
# underlying stock assets at 10:00 UTC, so the index pull MUST run after both — otherwise a container
# serves a stale index for a full day (the "ND20" vs "ND100" lag).
# This stops the schedule from silently regressing to the old 00:50 slot.
class RecurringScheduleTest < ActiveSupport::TestCase
  SCHEDULE = YAML.load_file(Rails.root.join('config/recurring.yml')).freeze

  # data-api's sync_fundamentals starts at 10:00 UTC and publishes the ND100 index once the members'
  # caps are in; the index pull must start strictly after it.
  DATA_API_ND100_REFRESH = 10 * 60 # minutes since midnight UTC

  %w[production development].each do |env|
    test "#{env}: deltabadger index sync runs after the stock-asset sync and data-api's ND100 refresh" do
      tasks = SCHEDULE.fetch(env)
      index_at = cron_minutes(tasks.dig('sync_indices_from_coingecko_job', 'schedule'))
      stock_at = cron_minutes(tasks.dig('sync_stocks_from_deltabadger_job', 'schedule'))

      assert_operator index_at, :>, stock_at,
                      "index sync (#{fmt(index_at)}) must run after the stock-asset sync (#{fmt(stock_at)})"
      assert_operator index_at, :>, DATA_API_ND100_REFRESH,
                      "index sync (#{fmt(index_at)}) must run after data-api's ND100 refresh (#{fmt(DATA_API_ND100_REFRESH)})"
    end

    # A bot waiting on a condition re-checks it on every minute boundary, and the conversion defers a bot
    # while one of its jobs is due. Run on the minute, the pass would find that job due every time and
    # never convert the bot; half a minute later the check has run and its next one is not due yet.
    test "#{env}: the single-asset conversion runs between minute boundaries" do
      schedule = SCHEDULE.fetch(env).dig('convert_single_asset_bots_job', 'schedule')
      runs = [Time.utc(2026, 9, 18, 20, 0, 0), Time.utc(2026, 9, 18, 20, 9, 59)].map do |from|
        Fugit.parse_cron(schedule).next_time(from).to_t.utc
      end

      assert(runs.all? { |at| at.sec.between?(15, 45) }, "#{schedule.inspect} runs at #{runs.map(&:iso8601)}")
    end
  end

  # Minutes-since-midnight for the minute+hour fields of a standard 5-field cron expression. Only
  # fixed minute/hour values are used for these daily jobs, so a wildcard/step is a misconfiguration.
  def cron_minutes(expression)
    assert_not_nil expression, 'schedule missing'
    minute, hour = expression.split.first(2)
    [minute, hour].each { |f| assert_match(/\A\d+\z/, f, "expected fixed minute/hour in #{expression.inspect}") }
    (hour.to_i * 60) + minute.to_i
  end

  def fmt(minutes)
    format('%<h>02d:%<m>02d UTC', h: minutes / 60, m: minutes % 60)
  end
end
