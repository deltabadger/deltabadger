require 'test_helper'

class GatewayAuthenticationTest < ActiveSupport::TestCase
  teardown do
    Doorkeeper::AccessToken.delete_all
    Doorkeeper::Application.destroy_all
  end

  test 'resolves user from valid bearer token' do
    admin = create(:user, admin: true)
    app = Doorkeeper::Application.create!(name: 'Test', redirect_uri: 'http://localhost/callback', confidential: false)
    token = Doorkeeper::AccessToken.create!(application: app, resource_owner_id: admin.id, token: SecureRandom.hex(32), expires_in: 3600)

    request = mock_request("Bearer #{token.token}")
    identifier = MCPTokenIdentifier.new(request)
    user = identifier.resolve

    assert_equal admin, user
  end

  test 'raises Unauthorized when no bearer token' do
    request = mock_request(nil)
    identifier = MCPTokenIdentifier.new(request)

    error = assert_raises(ActionMCP::GatewayIdentifier::Unauthorized) { identifier.resolve }
    assert_match(/Missing bearer token/, error.message)
  end

  test 'raises Unauthorized for invalid token' do
    request = mock_request('Bearer invalid_token')
    identifier = MCPTokenIdentifier.new(request)

    error = assert_raises(ActionMCP::GatewayIdentifier::Unauthorized) { identifier.resolve }
    assert_match(/Invalid access token/, error.message)
  end

  test 'raises Unauthorized for expired token' do
    admin = create(:user, admin: true)
    app = Doorkeeper::Application.create!(name: 'Test', redirect_uri: 'http://localhost/callback', confidential: false)
    token = Doorkeeper::AccessToken.create!(
      application: app, resource_owner_id: admin.id,
      token: SecureRandom.hex(32), expires_in: 0, created_at: 1.hour.ago
    )

    request = mock_request("Bearer #{token.token}")
    identifier = MCPTokenIdentifier.new(request)

    error = assert_raises(ActionMCP::GatewayIdentifier::Unauthorized) { identifier.resolve }
    assert_match(/expired/, error.message)
  end

  test 'raises Unauthorized for revoked token' do
    admin = create(:user, admin: true)
    app = Doorkeeper::Application.create!(name: 'Test', redirect_uri: 'http://localhost/callback', confidential: false)
    token = Doorkeeper::AccessToken.create!(
      application: app, resource_owner_id: admin.id,
      token: SecureRandom.hex(32), expires_in: 3600, revoked_at: Time.current
    )

    request = mock_request("Bearer #{token.token}")
    identifier = MCPTokenIdentifier.new(request)

    error = assert_raises(ActionMCP::GatewayIdentifier::Unauthorized) { identifier.resolve }
    assert_match(/revoked/, error.message)
  end

  private

  MockRequest = Struct.new(:env)

  def mock_request(authorization_header)
    env = {}
    env['HTTP_AUTHORIZATION'] = authorization_header if authorization_header
    MockRequest.new(env)
  end
end
