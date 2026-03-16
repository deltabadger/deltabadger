# frozen_string_literal: true

Doorkeeper.configure do
  orm :active_record

  # Devise integration
  resource_owner_authenticator do
    current_user || warden.authenticate!(scope: :user)
  end

  # OAuth 2.1: authorization_code only
  grant_flows %w[authorization_code]

  # PKCE required for all clients (OAuth 2.1)
  force_ssl_in_redirect_uri false # allow http for localhost dev
  force_pkce

  access_token_expires_in 1.hour
  use_refresh_token

  default_scopes :mcp

  # Always show consent screen — any client can self-register via DCR
  skip_authorization do
    false
  end

  # Custom base controller to avoid ApplicationController filters
  base_controller 'Oauth::BaseController'

  allow_blank_redirect_uri false
end
