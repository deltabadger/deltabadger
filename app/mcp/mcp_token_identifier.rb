# frozen_string_literal: true

class MCPTokenIdentifier < ActionMCP::GatewayIdentifier
  identifier :user
  authenticates :bearer_token

  # Errors are surfaced from the shared resolver. Messages are kept stable so
  # existing client behavior (and existing assertions) don't shift.
  ERROR_MESSAGES = {
    missing: 'Missing bearer token',
    invalid: 'Invalid access token',
    revoked: 'Access token revoked',
    expired: 'Access token expired',
    insufficient_scope: 'Access token missing required scope',
    user_not_found: 'User not found'
  }.freeze

  def resolve
    result = OauthBearerTokenResolver.call(
      authorization_header: @request.env['HTTP_AUTHORIZATION'],
      required_scope: 'mcp'
    )

    unless result.success?
      OauthClientContext.oauth_application = nil
      raise Unauthorized, ERROR_MESSAGES.fetch(result.error, 'Unauthorized')
    end

    # The gateway can only hand one identity to ActionMCP, and it has to be the
    # user. The client rides here instead, for the rest of this request.
    OauthClientContext.oauth_application = result.access_token.application
    result.user
  end
end
