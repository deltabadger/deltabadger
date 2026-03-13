# frozen_string_literal: true

class ApplicationMCPTool < ActionMCP::Tool
  abstract!

  # Defense-in-depth: check permissions even if tool_registry filtering
  # already hides disabled tools from the AI client.
  def call
    unless AppConfig.mcp_tool_enabled?(self.class.tool_name)
      @response = ActionMCP::ToolResponse.new
      @response.report_tool_error("Tool '#{self.class.tool_name}' is disabled. Enable it in Settings > MCP.")
      return @response
    end
    super
  end

  alias execute call
end
