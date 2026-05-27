# frozen_string_literal: true

# Resolves an `Authorization: Bearer <token>` header into a User, enforcing
# the required OAuth scope. Pure object — no controller, MCP, or session
# coupling — so it can be wired into both the REST controller layer and the
# MCP gateway identifier from the outside.
#
#   result = OauthBearerTokenResolver.call(
#     authorization_header: request.headers['Authorization'],
#     required_scope: 'api'
#   )
#   result.success?       # => true
#   result.user           # => #<User ...>
#   result.error          # => nil | :missing | :invalid | :revoked
#                         #    | :expired | :insufficient_scope | :user_not_found
class OauthBearerTokenResolver
  Result = Data.define(:user, :error) do
    def success? = error.nil?
  end

  BEARER_PATTERN = /\ABearer\s+(.+)\z/i

  def self.call(authorization_header:, required_scope:)
    new(authorization_header: authorization_header, required_scope: required_scope.to_s).call
  end

  def initialize(authorization_header:, required_scope:)
    @authorization_header = authorization_header
    @required_scope = required_scope
  end

  def call
    token_string = extract_bearer_token
    return failure(:missing) if token_string.blank?

    access_token = Doorkeeper::AccessToken.by_token(token_string)
    return failure(:invalid) unless access_token
    return failure(:revoked) if access_token.revoked?
    return failure(:expired) if access_token.expired?
    return failure(:insufficient_scope) unless scope_satisfied?(access_token)

    user = User.find_by(id: access_token.resource_owner_id)
    return failure(:user_not_found) unless user

    Result.new(user: user, error: nil)
  end

  private

  def extract_bearer_token
    return nil if @authorization_header.blank?

    match = BEARER_PATTERN.match(@authorization_header.to_s)
    return nil unless match

    match[1].strip.presence
  end

  def scope_satisfied?(access_token)
    access_token.scopes.exists?(@required_scope)
  end

  def failure(error)
    Result.new(user: nil, error: error)
  end
end
