# frozen_string_literal: true

class MCPTokenIdentifier < ActionMCP::GatewayIdentifier
  identifier :user
  authenticates :bearer_token

  def resolve
    token_string = extract_bearer_token
    raise Unauthorized, 'Missing bearer token' if token_string.blank?

    access_token = Doorkeeper::AccessToken.by_token(token_string)
    raise Unauthorized, 'Invalid access token' unless access_token
    raise Unauthorized, 'Access token revoked' if access_token.revoked?
    raise Unauthorized, 'Access token expired' if access_token.expired?

    user = User.find_by(id: access_token.resource_owner_id)
    raise Unauthorized, 'User not found' unless user

    user
  end
end
