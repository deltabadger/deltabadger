require "test_helper"

class CalculateRestartDelayTest < ActiveSupport::TestCase
  test "raises ArgumentError for 0 restarts" do
    assert_raises(ArgumentError) do
      CalculateRestartDelay.new.call(0)
    end
  end

  test "returns 15 minutes for 1 restart" do
    assert_equal 15.minutes, CalculateRestartDelay.new.call(1)
  end

  test "returns 30 minutes for 2 restarts" do
    assert_equal 30.minutes, CalculateRestartDelay.new.call(2)
  end

  test "returns 2 hours for 4 restarts" do
    assert_equal 2.hours, CalculateRestartDelay.new.call(4)
  end

  test "returns 8 hours for 6 restarts" do
    assert_equal 8.hours, CalculateRestartDelay.new.call(6)
  end
end
