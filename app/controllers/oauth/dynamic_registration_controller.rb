# frozen_string_literal: true

module Oauth
  class DynamicRegistrationController < ActionController::Base
    skip_forgery_protection

    ALLOWED_SCOPES = %w[mcp api].freeze
    DEFAULT_SCOPE = 'mcp'

    # RFC 7591: Dynamic Client Registration
    # POST /oauth/register — no auth required (per spec)
    def create
      redirect_uris = params[:redirect_uris]

      if redirect_uris.blank?
        return render json: { error: 'invalid_client_metadata', error_description: 'redirect_uris is required' },
                      status: :bad_request
      end

      scopes = normalize_scopes(params[:scope])
      if scopes.nil?
        return render json: {
          error: 'invalid_client_metadata',
          error_description: "scope must be a subset of: #{ALLOWED_SCOPES.join(' ')}"
        }, status: :bad_request
      end

      redirect_uri = Array(redirect_uris).join("\n")
      client_name = params[:client_name].presence || 'MCP Client'

      application = Doorkeeper::Application.create!(
        name: client_name,
        redirect_uri: redirect_uri,
        confidential: false,
        scopes: scopes,
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
        scope: scopes
      }, status: :created
    end

    private

    # Returns the normalized scope string, or nil if the input contains any
    # token outside ALLOWED_SCOPES. Absent/blank input falls back to DEFAULT_SCOPE.
    def normalize_scopes(raw)
      return DEFAULT_SCOPE if raw.blank?

      tokens = raw.to_s.split.uniq
      return nil if tokens.empty?
      return nil unless (tokens - ALLOWED_SCOPES).empty?

      # Canonical order: stable across `'api mcp'` vs `'mcp api'` so storage
      # and reflected response match regardless of input order.
      tokens.sort.join(' ')
    end
  end
end
