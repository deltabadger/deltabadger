# frozen_string_literal: true

# Authenticates REST API requests via an OAuth `:api`-scoped bearer token.
# No session-auth fallback: a signed-in browser session without a bearer
# header is rejected exactly like an unauthenticated request. This is
# intentional — REST is OAuth-only.
module ApiOauthAuthentication
  extend ActiveSupport::Concern

  ERROR_CODES = {
    missing: 'missing_token',
    invalid: 'invalid_token',
    revoked: 'token_revoked',
    expired: 'token_expired',
    insufficient_scope: 'insufficient_scope',
    user_not_found: 'user_not_found'
  }.freeze

  ERROR_MESSAGES = {
    missing: 'Missing bearer token',
    invalid: 'Invalid access token',
    revoked: 'Access token revoked',
    expired: 'Access token expired',
    insufficient_scope: 'Access token missing required scope',
    user_not_found: 'User not found'
  }.freeze

  included do
    before_action :authenticate_api_user!
    attr_reader :current_user
  end

  private

  def authenticate_api_user!
    result = OauthBearerTokenResolver.call(
      authorization_header: request.headers['Authorization'],
      required_scope: 'api'
    )

    if result.success?
      @current_user = result.user
      return
    end

    render_api_auth_error(result.error)
  end

  def render_api_auth_error(error)
    code = ERROR_CODES.fetch(error, 'unauthorized')
    message = ERROR_MESSAGES.fetch(error, 'Unauthorized')
    status = error == :insufficient_scope ? :forbidden : :unauthorized

    render json: { data: nil, error: { code: code, message: message } }, status: status
  end
end
