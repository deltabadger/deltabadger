require 'test_helper'

# Guards the cross-repo ordering that bit us once: hosted containers pull the deltabadger-sourced
# Nasdaq index in `sync_indices_from_coingecko_job`. data-api refreshes that index at 07:30 UTC and
# the container syncs the underlying stock assets at 10:00 UTC, so the index pull MUST run after both
# — otherwise a container serves a stale index for a full day (the "Nasdaq 10" vs "Nasdaq 20" lag).
# This stops the schedule from silently regressing to the old 00:50 slot.
class RecurringScheduleTest < ActiveSupport::TestCase
  SCHEDULE = YAML.load_file(Rails.root.join('config/recurring.yml')).freeze

  # data-api's sync_nasdaq_index runs at 07:30 UTC; the index pull must start strictly after it.
  DATA_API_NASDAQ_REFRESH = (7 * 60) + 30 # minutes since midnight UTC

  %w[production development].each do |env|
    test "#{env}: deltabadger index sync runs after the stock-asset sync and data-api's Nasdaq refresh" do
      tasks = SCHEDULE.fetch(env)
      index_at = cron_minutes(tasks.dig('sync_indices_from_coingecko_job', 'schedule'))
      stock_at = cron_minutes(tasks.dig('sync_stocks_from_deltabadger_job', 'schedule'))

      assert_operator index_at, :>, stock_at,
                      "index sync (#{fmt(index_at)}) must run after the stock-asset sync (#{fmt(stock_at)})"
      assert_operator index_at, :>, DATA_API_NASDAQ_REFRESH,
                      "index sync (#{fmt(index_at)}) must run after data-api's Nasdaq refresh (#{fmt(DATA_API_NASDAQ_REFRESH)})"
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
