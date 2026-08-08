# frozen_string_literal: true

class ApplicationGateway < ActionMCP::Gateway
  identified_by MCPTokenIdentifier

  # Runs on every authenticated request, so the registry cannot go stale — and it
  # is a real boundary, not a listing filter: actionmcp rejects a tools/call for
  # anything outside it. ApplicationMCPTool still checks per call, because this is
  # a persisted column and the cheaper of the two things to get wrong.
  def configure_session(session)
    session.session_data = { 'user_id' => user.id }
    session.tool_registry = ToolAccess.new(
      user: user, application: OauthClientContext.oauth_application
    ).enabled_mcp_tool_names
  end
end
