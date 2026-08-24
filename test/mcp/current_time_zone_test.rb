require 'test_helper'

# ActionMCP::Current sets Time.zone to the calling user's zone. Current attributes are reset after
# every request — the thread-local zone has to go back with them, or it stays on the thread and
# every request served by it afterwards reads the clock in a stranger's zone.
class MCPCurrentTimeZoneTest < ActiveSupport::TestCase
  teardown do
    ActionMCP::Current.reset
    Time.zone = Time.zone_default
  end

  test 'resetting the MCP request context puts the zone back' do
    ActionMCP::Current.user = create(:user, time_zone: 'Tallinn')

    assert_equal 'Tallinn', Time.zone.name, 'guard: the gem is still setting the zone from the user'

    ActionMCP::Current.reset

    assert_equal Time.zone_default, Time.zone
  end
end
