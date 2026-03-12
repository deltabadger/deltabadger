# frozen_string_literal: true

# Rack middleware that authenticates MCP requests via a secret token in the URL path.
# Requests must match /<token>/... to reach the MCP server. The token prefix is
# stripped before passing to ActionMCP so its routes (mounted at "/") work normally.
class MCPSecretPathAuth
  def initialize(app)
    @app = app
  end

  def call(env)
    token = AppConfig.mcp_access_token
    return not_found if token.blank?

    path = env['PATH_INFO'] || '/'
    prefix = "/#{token}"

    return not_found unless path == prefix || path.start_with?("#{prefix}/")

    env['PATH_INFO'] = path.delete_prefix(prefix)
    env['PATH_INFO'] = '/' if env['PATH_INFO'].empty?
    env['SCRIPT_NAME'] = "#{env['SCRIPT_NAME']}#{prefix}"

    @app.call(env)
  end

  private

  def not_found
    [404, { 'content-type' => 'text/plain' }, ['Not Found']]
  end
end
