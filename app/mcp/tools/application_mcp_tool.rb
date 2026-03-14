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

  private

  def with_dry_run_if_enabled
    if AppConfig.mcp_dry_run?
      Thread.current[:force_dry_run] = true
      begin
        yield
      ensure
        Thread.current[:force_dry_run] = nil
      end
    else
      yield
    end
  end
end
