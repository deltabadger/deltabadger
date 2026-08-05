# frozen_string_literal: true

module Oauth
  class DynamicRegistrationController < ActionController::Base
    skip_forgery_protection

    ALLOWED_SCOPES = %w[mcp api].freeze
    DEFAULT_SCOPE = 'mcp'
    MAX_CLIENT_NAME_LENGTH = 100
    MAX_REDIRECT_URIS = 5
    MAX_REDIRECT_URI_LENGTH = 2_000

    # RFC 7591: Dynamic Client Registration
    # POST /oauth/register — no auth required (per spec)
    def create
      redirect_uris = Array(params[:redirect_uris])

      if redirect_uris.blank?
        return render json: { error: 'invalid_client_metadata',
                              error_description: 'redirect_uris is required' },
                      status: :bad_request
      end

      if redirect_uris.size > MAX_REDIRECT_URIS
        return render json: { error: 'invalid_client_metadata',
                              error_description: "at most #{MAX_REDIRECT_URIS} redirect_uris" },
                      status: :bad_request
      end

      if redirect_uris.any? { |uri| uri.to_s.length > MAX_REDIRECT_URI_LENGTH }
        return render json: { error: 'invalid_client_metadata',
                              error_description: "each redirect_uri must be at most #{MAX_REDIRECT_URI_LENGTH} characters" },
                      status: :bad_request
      end

      unless redirect_uris.all? { |uri| absolute_http_uri?(uri) }
        return render json: { error: 'invalid_redirect_uri',
                              error_description: 'redirect_uris must be absolute http(s) URLs' },
                      status: :bad_request
      end

      scopes = normalize_scopes(params[:scope])
      if scopes.nil?
        return render json: {
          error: 'invalid_client_metadata',
          error_description: "scope must be a subset of: #{ALLOWED_SCOPES.join(' ')}"
        }, status: :bad_request
      end

      redirect_uri = redirect_uris.join("\n")
      client_name = params[:client_name].to_s.strip.presence&.first(MAX_CLIENT_NAME_LENGTH) || 'MCP Client'

      application = begin
        Doorkeeper::Application.create!(
          name: client_name,
          redirect_uri: redirect_uri,
          confidential: false,
          scopes: scopes,
          registration_access_token: SecureRandom.hex(32),
          token_endpoint_auth_method: 'none',
          grant_types: 'authorization_code',
          response_types: 'code'
        )
      rescue ActiveRecord::RecordInvalid => e
        return render json: { error: 'invalid_redirect_uri', error_description: e.record.errors.full_messages.join(', ') },
                      status: :bad_request
      end

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

    # Doorkeeper's own validator already rejects javascript:, data:, relative URIs and
    # fragments, but it ACCEPTS custom and native schemes (myapp://cb, com.evil.app:/cb,
    # urn:ietf:wg:oauth:2.0:oob). This app's clients are web clients, and the host is
    # what the owner is shown on the consent screen, so require a real http(s) URL.
    def absolute_http_uri?(value)
      uri = URI.parse(value.to_s)
      uri.absolute? && %w[http https].include?(uri.scheme) && uri.host.present?
    rescue URI::InvalidURIError
      false
    end
  end
end
