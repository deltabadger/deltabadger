# frozen_string_literal: true

# What one OAuth client is allowed to do on behalf of one user.
#
# Permissions used to hang off the user alone, which meant every client the user
# had ever authorized shared one set — including tools switched on long after the
# client was connected. This record is the client half of that decision: the tool
# names the user granted at consent, frozen at that moment. It never widens on its
# own; ToolAccess intersects it with the user's current setting on every call.
class ConnectedClient < ApplicationRecord
  belongs_to :user
  belongs_to :oauth_application, class_name: 'Doorkeeper::Application'

  def self.for(user:, application:)
    return nil if user.nil? || application.nil?

    find_by(user_id: user.id, oauth_application_id: application.id)
  end

  # Filtered on read, not on write: a tool that is renamed or removed leaves its old
  # name sitting in every grant that mentioned it, and this drops it.
  #
  # INVARIANT, and the filter depends on it: a tool name is a permanent identifier.
  # Never reuse a retired name for a different capability. Filtering makes a stale
  # grant inert, but it cannot tell "gone" from "gone and later reissued to mean
  # something else" — reuse would silently re-arm every old grant that mentioned it.
  # If a tool is ever renamed, retire the old name for good. AppConfig's tool
  # constants carry the same note.
  def granted_mcp_tools
    Array(mcp_tools).map(&:to_s) & AppConfig::MCP_TOOL_DEFAULTS.keys
  end

  def granted_rest_tools
    Array(rest_tools).map(&:to_s) & AppConfig::REST_TOOL_DEFAULTS.keys
  end
end
