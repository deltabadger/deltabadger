# frozen_string_literal: true

# Rack middleware that returns a proper OAuth 2.1 challenge (RFC 9728) for
# unauthenticated requests to the MCP endpoint.
#
# Claude.ai and other MCP clients expect:
#   HTTP/1.1 401 Unauthorized
#   WWW-Authenticate: Bearer resource_metadata="<protected-resource-url>"
#
# Without this header, clients cannot discover the OAuth flow automatically.
class McpOauthChallenge
  def initialize(app)
    @app = app
  end

  def call(env)
    path = env['PATH_INFO'] || '/'
    return @app.call(env) unless path.start_with?('/mcp')

    auth_header = env['HTTP_AUTHORIZATION'] || ''
    has_bearer = auth_header.match?(/\ABearer\s+\S/i)

    return @app.call(env) if has_bearer

    base_url = "#{env['rack.url_scheme']}://#{env['HTTP_HOST']}"
    resource_metadata = "#{base_url}/.well-known/oauth-protected-resource"

    [
      401,
      {
        'WWW-Authenticate' => "Bearer resource_metadata=\"#{resource_metadata}\"",
        'Content-Type' => 'application/json'
      },
      ['{"error":"unauthorized","error_description":"Bearer token required"}']
    ]
  end
end
