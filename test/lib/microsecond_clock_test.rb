require 'test_helper'

# Pins the test clock's precision to SQLite's (see Time.now in test_helper.rb), so a Linux
# clock with nanoseconds cannot make an in-memory time differ from its reloaded copy. The Linux
# clock is simulated by replacing the raw clock under the wrapper; travel_back in teardown restores it.
class MicrosecondClockTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

  setup do
    simple_stubs.stub_object(Time, :nanosecond_now) { Time.at(1_791_000_000, 631_779_349, :nsec) }
  end

  test 'a nanosecond clock reading is truncated to microseconds' do
    assert_equal 631_779_000, Time.now.nsec
    assert_equal 631_779_000, Time.current.nsec
  end

  test 'a time that went through the database equals the one in memory' do
    user = create(:user)

    assert_equal user.attributes, User.find(user.id).attributes
  end
end
