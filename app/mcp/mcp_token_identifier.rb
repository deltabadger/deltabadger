# frozen_string_literal: true

class MCPTokenIdentifier < ActionMCP::GatewayIdentifier
  identifier :user
  authenticates :bearer_token

  def resolve
    # Authentication is handled by the secret URL middleware in mcp/config.ru.
    # This identifier only resolves the admin user for ActionMCP::Current.user.
    admin = User.find_by(admin: true)
    raise Unauthorized, 'No admin user found' unless admin

    admin
  end
end
