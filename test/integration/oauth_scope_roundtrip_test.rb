# frozen_string_literal: true

require 'test_helper'

# Verifies that scopes requested at /oauth/authorize survive the full
# consent → code → token exchange and end up on the issued AccessToken.
# This is the test the plan calls out: "no silent stripping" through the
# round-trip, particularly for the new `:api` scope landing in slice 4.
class OauthScopeRoundtripTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  CODE_VERIFIER = 'a-very-long-pkce-code-verifier-that-exceeds-the-minimum-43-chars'

  setup do
    @user = create(:user)
    sign_in @user

    @application = Doorkeeper::Application.create!(
      name: 'Test Client',
      redirect_uri: 'http://localhost/callback',
      confidential: false,
      scopes: 'mcp api',
      token_endpoint_auth_method: 'none',
      grant_types: 'authorization_code',
      response_types: 'code'
    )
  end

  teardown do
    Doorkeeper::AccessToken.delete_all
    Doorkeeper::AccessGrant.delete_all
    Doorkeeper::Application.destroy_all
  end

  test 'requested api mcp scope survives the authorization round-trip' do
    issued = run_authorization_flow(requested_scope: 'api mcp')

    assert_equal %w[api mcp], issued.scopes.to_a.sort
  end

  test 'requested mcp api scope survives the authorization round-trip regardless of order' do
    issued = run_authorization_flow(requested_scope: 'mcp api')

    assert_equal %w[api mcp], issued.scopes.to_a.sort
  end

  test 'requested api-only scope produces an api-only access token' do
    issued = run_authorization_flow(requested_scope: 'api')

    assert_equal ['api'], issued.scopes.to_a
  end

  private

  def code_challenge
    Base64.urlsafe_encode64(Digest::SHA256.digest(CODE_VERIFIER), padding: false)
  end

  def run_authorization_flow(requested_scope:)
    # POST /oauth/authorize directly with use: 1 — Doorkeeper treats this as
    # consent-given (skips rendering the new.html.erb form) and issues an
    # authorization code via 302 to the redirect_uri.
    post '/oauth/authorize', params: {
      client_id: @application.uid,
      redirect_uri: @application.redirect_uri,
      response_type: 'code',
      scope: requested_scope,
      code_challenge: code_challenge,
      code_challenge_method: 'S256'
    }

    assert_response :redirect, "expected redirect after authorize, got #{response.status}: #{response.body[0, 500]}"
    location = response.headers['Location']
    code = CGI.parse(URI.parse(location).query)['code'].first
    assert code.present?, "no authorization code in redirect: #{location}"

    post '/oauth/token', params: {
      grant_type: 'authorization_code',
      client_id: @application.uid,
      redirect_uri: @application.redirect_uri,
      code: code,
      code_verifier: CODE_VERIFIER
    }

    assert_response :success, "token exchange failed: #{response.body}"
    token_json = JSON.parse(response.body)
    Doorkeeper::AccessToken.by_token(token_json['access_token'])
  end
end
