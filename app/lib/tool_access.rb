# frozen_string_literal: true

# Decides whether one OAuth client may call one tool on behalf of one user.
#
# Two things have to be true, and both are checked on every call rather than
# cached anywhere: the user has the tool enabled *now*, and the client was granted
# it at consent. Switching a tool off globally therefore takes effect immediately,
# everywhere, including mid-session; and a client cannot pick up a tool the user
# enables after the fact.
#
# This is the only place a tool permission is decided. All three enforcement
# points — the MCP session registry, the per-call gate in ApplicationMCPTool, and
# require_rest_tool! in the REST API — come through here.
class ToolAccess
  def initialize(user:, application:)
    @user = user
    @application = application
  end

  def mcp_enabled?(tool_name)
    return false unless @user
    return @user.mcp_tool_enabled?(tool_name) if acting_as_self?

    granted_mcp_tools.include?(tool_name) && @user.mcp_tool_enabled?(tool_name)
  end

  def rest_enabled?(tool_name)
    return false unless @user
    return @user.rest_tool_enabled?(tool_name) if acting_as_self?

    granted_rest_tools.include?(tool_name) && @user.rest_tool_enabled?(tool_name)
  end

  def enabled_mcp_tool_names
    return [] unless @user
    return @user.enabled_mcp_tool_names if acting_as_self?

    @user.enabled_mcp_tool_names & granted_mcp_tools
  end

  private

  # The personal API token is minted by the user for the user: no authorize
  # round-trip, no consent screen, no third party to restrict. There is nothing to
  # intersect with, so the user's own setting is the whole answer.
  #
  # The owner check is load-bearing, not decoration. Without it the exemption keys
  # on a boolean that says "some user's personal application", and any token issued
  # against any personal application would skip the grant entirely. Doorkeeper's
  # allow_grant_flow_for_client refuses these applications an OAuth flow for the
  # same reason; this is the second lock on the same door.
  def acting_as_self?
    return false unless @application&.personal_access_token?

    @application.personal_owner_id.present? && @application.personal_owner_id == @user&.id
  end

  def connected_client
    return @connected_client if defined?(@connected_client)

    @connected_client = ConnectedClient.for(user: @user, application: @application)
  end

  def granted_mcp_tools
    @granted_mcp_tools ||= connected_client&.granted_mcp_tools || []
  end

  def granted_rest_tools
    @granted_rest_tools ||= connected_client&.granted_rest_tools || []
  end
end
