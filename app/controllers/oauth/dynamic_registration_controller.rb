# frozen_string_literal: true

module Oauth
  class DynamicRegistrationController < ActionController::Base
    skip_forgery_protection

    # RFC 7591: Dynamic Client Registration
    # POST /oauth/register — no auth required (per spec)
    def create
      redirect_uris = params[:redirect_uris]

      if redirect_uris.blank?
        return render json: { error: 'invalid_client_metadata', error_description: 'redirect_uris is required' },
                      status: :bad_request
      end

      redirect_uri = Array(redirect_uris).join("\n")
      client_name = params[:client_name].presence || 'MCP Client'

      application = Doorkeeper::Application.create!(
        name: client_name,
        redirect_uri: redirect_uri,
        confidential: false,
        scopes: 'mcp',
        registration_access_token: SecureRandom.hex(32),
        token_endpoint_auth_method: 'none',
        grant_types: 'authorization_code',
        response_types: 'code'
      )

      render json: {
        client_id: application.uid,
        client_name: application.name,
        redirect_uris: application.redirect_uri.split("\n"),
        registration_access_token: application.registration_access_token,
        token_endpoint_auth_method: 'none',
        grant_types: %w[authorization_code],
        response_types: %w[code],
        scope: 'mcp'
      }, status: :created
    end
  end
end
