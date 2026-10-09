# frozen_string_literal: true

class ApplicationMCPTool < ActionMCP::Tool
  abstract!

  # Paper trading simulates a single order inside the call. These tools instead leave something
  # running that trades or withdraws on its own later, outside any call, so there is nothing to
  # simulate: with paper trading on they refuse before touching anything.
  LIVE_AUTOMATION_TOOLS = %w[create_bot create_index_bot create_signal_bot start_bot start_rule].freeze

  # Defense-in-depth: the session registry already hides tools this client may not
  # use, but that registry is written once per request and persisted, so the
  # decision is re-made here on every call.
  #
  # The two refusals stay distinct on purpose. "Disabled" and "not granted to this
  # client" have different fixes in different places in Settings, and collapsing
  # them sends the owner to the wrong screen.
  def call
    tool = self.class.tool_name

    return refuse("Tool '#{tool}' is disabled. Enable it in Settings > MCP.") unless current_user&.mcp_tool_enabled?(tool)
    return refuse("Tool '#{tool}' is not available to this client. Grant it in Settings > Connect.") unless tool_access.mcp_enabled?(tool)
    if current_user.mcp_dry_run? && LIVE_AUTOMATION_TOOLS.include?(tool)
      return refuse("[DRY RUN] Paper trading is on, so nothing was created or started: '#{tool}' would leave a bot " \
                    'or rule running that moves real money. Turn Paper Trading off in Settings > MCP to use it.')
    end

    super
  end

  alias execute call

  private

  def refuse(message)
    @response = ActionMCP::ToolResponse.new
    @response.report_tool_error(message)
    @response
  end

  # Not memoized: the tool instance is per call, and the permission must reflect
  # the database at the moment of the call.
  def tool_access
    ToolAccess.new(user: current_user, application: OauthClientContext.oauth_application)
  end
end
