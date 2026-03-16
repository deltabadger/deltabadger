# frozen_string_literal: true

module Oauth
  class WellKnownController < ActionController::Base
    # RFC 9728: OAuth Protected Resource Metadata
    def oauth_protected_resource
      render json: {
        resource: "#{request.base_url}/mcp",
        authorization_servers: [request.base_url],
        bearer_methods_supported: %w[header]
      }
    end

    # RFC 8414: OAuth Authorization Server Metadata
    def oauth_authorization_server
      render json: {
        issuer: request.base_url,
        authorization_endpoint: "#{request.base_url}/oauth/authorize",
        token_endpoint: "#{request.base_url}/oauth/token",
        registration_endpoint: "#{request.base_url}/oauth/register",
        revocation_endpoint: "#{request.base_url}/oauth/revoke",
        scopes_supported: %w[mcp],
        response_types_supported: %w[code],
        grant_types_supported: %w[authorization_code refresh_token],
        token_endpoint_auth_methods_supported: %w[none],
        code_challenge_methods_supported: %w[S256]
      }
    end
  end
end
