# frozen_string_literal: true

# ActionMCP::Current sets Time.zone to the calling user's zone (ActionMCP::Current#user=) and never
# puts it back. Current attributes are reset after every request; the thread-local zone is not one
# of them, so on a single-threaded server the zone one MCP call left behind stayed on the thread
# for every request after it. Reset it with the rest of the attributes.
Rails.application.config.to_prepare do
  ActionMCP::Current.resets { Time.zone = nil }
end
