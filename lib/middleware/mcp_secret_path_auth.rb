# frozen_string_literal: true

# Rack middleware that intercepts MCP requests authenticated via a secret token
# in the URL path. Requests matching /<token>/... are routed to ActionMCP.server;
# all other requests pass through to the main Rails application.
#
# The token prefix is stripped before passing to ActionMCP so its routes
# (mounted at "/") work normally.
class MCPSecretPathAuth
  def initialize(app)
    @app = app
  end

  def call(env)
    token = AppConfig.mcp_access_token
    return @app.call(env) if token.blank?

    path = env['PATH_INFO'] || '/'
    prefix = "/#{token}"

    return @app.call(env) unless path == prefix || path.start_with?("#{prefix}/")

    env['PATH_INFO'] = path.delete_prefix(prefix)
    env['PATH_INFO'] = '/' if env['PATH_INFO'].empty?
    env['SCRIPT_NAME'] = "#{env['SCRIPT_NAME']}#{prefix}"

    mcp_server.call(env)
  end

  private

  def mcp_server
    @mcp_server ||= ActionMCP.server
  end
end
