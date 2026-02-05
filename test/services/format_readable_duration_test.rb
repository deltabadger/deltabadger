require 'test_helper'

class FormatReadableDurationTest < ActiveSupport::TestCase
  test 'formats 0 seconds' do
    assert_equal '0 seconds', FormatReadableDuration.new.call(0)
  end

  test 'formats 60 seconds as 1 minute' do
    assert_equal '1 minute', FormatReadableDuration.new.call(60)
  end

  test 'formats 15 minutes' do
    assert_equal '15 minutes', FormatReadableDuration.new.call(15.minutes)
  end

  test 'formats 1 hour' do
    assert_equal '1 hour', FormatReadableDuration.new.call(1.hour)
  end

  test 'formats four 15-minute durations as 1 hour' do
    assert_equal '1 hour', FormatReadableDuration.new.call(15.minutes * 4)
  end

  test 'formats five 15-minute durations as 1 hour and 15 minutes' do
    assert_equal '1 hour and 15 minutes', FormatReadableDuration.new.call(15.minutes * 5)
  end
end
