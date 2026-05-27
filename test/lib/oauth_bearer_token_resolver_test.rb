# frozen_string_literal: true

require 'test_helper'

class OauthBearerTokenResolverTest < ActiveSupport::TestCase
  teardown do
    Doorkeeper::AccessToken.delete_all
    Doorkeeper::Application.destroy_all
  end

  # ---- valid ----------------------------------------------------------------

  test 'resolves user from a valid bearer token with the required scope' do
    user = create(:user, admin: true)
    token = create_token(user: user, scopes: 'mcp')

    result = OauthBearerTokenResolver.call(
      authorization_header: "Bearer #{token.token}",
      required_scope: 'mcp'
    )

    assert_nil result.error
    assert_equal user, result.user
  end

  test 'accepts a token whose scopes include the required scope alongside others' do
    user = create(:user)
    token = create_token(user: user, scopes: 'mcp api')

    api = OauthBearerTokenResolver.call(authorization_header: "Bearer #{token.token}", required_scope: 'api')
    mcp = OauthBearerTokenResolver.call(authorization_header: "Bearer #{token.token}", required_scope: 'mcp')

    assert_nil api.error
    assert_equal user, api.user
    assert_nil mcp.error
    assert_equal user, mcp.user
  end

  test 'Result#success? is true when error is nil and false otherwise' do
    user = create(:user)
    token = create_token(user: user, scopes: 'mcp')

    ok = OauthBearerTokenResolver.call(authorization_header: "Bearer #{token.token}", required_scope: 'mcp')
    bad = OauthBearerTokenResolver.call(authorization_header: nil, required_scope: 'mcp')

    assert ok.success?
    assert_not bad.success?
  end

  # ---- missing --------------------------------------------------------------

  test 'returns :missing when authorization header is nil' do
    result = OauthBearerTokenResolver.call(authorization_header: nil, required_scope: 'mcp')

    assert_equal :missing, result.error
    assert_nil result.user
  end

  test 'returns :missing when authorization header is blank' do
    result = OauthBearerTokenResolver.call(authorization_header: '', required_scope: 'mcp')

    assert_equal :missing, result.error
    assert_nil result.user
  end

  test 'returns :missing when authorization header is not a Bearer scheme' do
    result = OauthBearerTokenResolver.call(
      authorization_header: 'Basic dXNlcjpwYXNz',
      required_scope: 'mcp'
    )

    assert_equal :missing, result.error
    assert_nil result.user
  end

  test 'returns :missing when Bearer scheme is present but token portion is empty' do
    result = OauthBearerTokenResolver.call(authorization_header: 'Bearer ', required_scope: 'mcp')

    assert_equal :missing, result.error
    assert_nil result.user
  end

  # ---- invalid --------------------------------------------------------------

  test 'returns :invalid when the bearer token does not match any stored token' do
    result = OauthBearerTokenResolver.call(
      authorization_header: 'Bearer this_token_does_not_exist',
      required_scope: 'mcp'
    )

    assert_equal :invalid, result.error
    assert_nil result.user
  end

  # ---- revoked --------------------------------------------------------------

  test 'returns :revoked when the access token has been revoked' do
    user = create(:user)
    token = create_token(user: user, scopes: 'mcp', revoked_at: Time.current)

    result = OauthBearerTokenResolver.call(
      authorization_header: "Bearer #{token.token}",
      required_scope: 'mcp'
    )

    assert_equal :revoked, result.error
    assert_nil result.user
  end

  # ---- expired --------------------------------------------------------------

  test 'returns :expired when the access token has expired' do
    user = create(:user)
    token = create_token(user: user, scopes: 'mcp', expires_in: 0, created_at: 1.hour.ago)

    result = OauthBearerTokenResolver.call(
      authorization_header: "Bearer #{token.token}",
      required_scope: 'mcp'
    )

    assert_equal :expired, result.error
    assert_nil result.user
  end

  test 'revoked takes precedence over expired when both apply' do
    # Defensive: if a token is both revoked and expired, surface :revoked
    # (matches the current MCPTokenIdentifier check order so the next slice can
    # swap in the resolver without changing observable behavior).
    user = create(:user)
    token = create_token(
      user: user, scopes: 'mcp',
      expires_in: 0, created_at: 1.hour.ago,
      revoked_at: Time.current
    )

    result = OauthBearerTokenResolver.call(
      authorization_header: "Bearer #{token.token}",
      required_scope: 'mcp'
    )

    assert_equal :revoked, result.error
  end

  # ---- insufficient_scope ---------------------------------------------------

  test 'returns :insufficient_scope when the token lacks the required scope' do
    user = create(:user)
    token = create_token(user: user, scopes: 'mcp')

    result = OauthBearerTokenResolver.call(
      authorization_header: "Bearer #{token.token}",
      required_scope: 'api'
    )

    assert_equal :insufficient_scope, result.error
    assert_nil result.user
  end

  test 'returns :insufficient_scope when the token has only api but mcp is required' do
    user = create(:user)
    token = create_token(user: user, scopes: 'api')

    result = OauthBearerTokenResolver.call(
      authorization_header: "Bearer #{token.token}",
      required_scope: 'mcp'
    )

    assert_equal :insufficient_scope, result.error
    assert_nil result.user
  end

  test 'returns :insufficient_scope when token has no scopes at all' do
    user = create(:user)
    token = create_token(user: user, scopes: '')

    result = OauthBearerTokenResolver.call(
      authorization_header: "Bearer #{token.token}",
      required_scope: 'api'
    )

    assert_equal :insufficient_scope, result.error
    assert_nil result.user
  end

  # ---- user_not_found -------------------------------------------------------

  test 'returns :user_not_found when the resource owner no longer exists' do
    user = create(:user)
    token = create_token(user: user, scopes: 'mcp')
    # Bypass `dependent: :destroy` on access tokens — we want the token to
    # survive so the resolver can hit the user_not_found branch.
    User.where(id: user.id).delete_all

    result = OauthBearerTokenResolver.call(
      authorization_header: "Bearer #{token.token}",
      required_scope: 'mcp'
    )

    assert_equal :user_not_found, result.error
    assert_nil result.user
  end

  test ':user_not_found is distinguishable from :invalid' do
    # The plan calls this out explicitly: token-owner-deleted is an auth
    # failure but must not look like a forged token in logs/metrics.
    user = create(:user)
    token = create_token(user: user, scopes: 'mcp')
    # Bypass `dependent: :destroy` on access tokens — we want the token to
    # survive so the resolver can hit the user_not_found branch.
    User.where(id: user.id).delete_all

    deleted_owner = OauthBearerTokenResolver.call(
      authorization_header: "Bearer #{token.token}", required_scope: 'mcp'
    )
    bogus = OauthBearerTokenResolver.call(
      authorization_header: 'Bearer nope_nope_nope', required_scope: 'mcp'
    )

    assert_equal :user_not_found, deleted_owner.error
    assert_equal :invalid, bogus.error
  end

  # ---- ordering -------------------------------------------------------------

  test 'scope is enforced before user lookup' do
    # If a token is otherwise valid but lacks the required scope AND the
    # resource owner is gone, surface :insufficient_scope — scope is the
    # earlier guard. Keeps reasoning about precedence consistent.
    user = create(:user)
    token = create_token(user: user, scopes: 'mcp')
    # Bypass `dependent: :destroy` on access tokens — we want the token to
    # survive so the resolver can hit the user_not_found branch.
    User.where(id: user.id).delete_all

    result = OauthBearerTokenResolver.call(
      authorization_header: "Bearer #{token.token}",
      required_scope: 'api'
    )

    assert_equal :insufficient_scope, result.error
  end

  test 'revocation is checked before scope' do
    user = create(:user)
    token = create_token(user: user, scopes: 'mcp', revoked_at: Time.current)

    result = OauthBearerTokenResolver.call(
      authorization_header: "Bearer #{token.token}",
      required_scope: 'api' # scope would also fail, but :revoked must win
    )

    assert_equal :revoked, result.error
  end

  # ---- personal API token (slice 10) ---------------------------------------

  test 'a token minted by User#personal_api_token resolves successfully against required_scope: "api"' do
    user = create(:user)
    token = user.personal_api_token # uses the new Personal API Token path

    result = OauthBearerTokenResolver.call(
      authorization_header: "Bearer #{token.token}",
      required_scope: 'api'
    )

    assert_nil result.error
    assert_equal user, result.user
  end

  test 'a personal token is rejected at the /mcp scope (it carries only :api)' do
    user = create(:user)
    token = user.personal_api_token

    result = OauthBearerTokenResolver.call(
      authorization_header: "Bearer #{token.token}",
      required_scope: 'mcp'
    )

    assert_equal :insufficient_scope, result.error
  end

  private

  def create_token(user:, scopes: 'mcp', **attrs)
    app = Doorkeeper::Application.create!(
      name: "Test #{SecureRandom.hex(4)}",
      redirect_uri: 'http://localhost/callback',
      confidential: false
    )
    Doorkeeper::AccessToken.create!(
      {
        application: app,
        resource_owner_id: user.id,
        token: SecureRandom.hex(32),
        scopes: scopes,
        expires_in: 3600
      }.merge(attrs)
    )
  end
end
