# frozen_string_literal: true

require 'test_helper'

# A client connected without a stored tool list follows the user's REST switches, live. The
# token's scope still decides which surface it can reach at all.
class Api::V1::FollowingClientTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user)
    @oauth_app = Doorkeeper::Application.create!(
      name: 'Follower', redirect_uri: 'http://localhost/callback', confidential: false, scopes: 'mcp api'
    )
    ConnectedClient.create!(user: @user, oauth_application: @oauth_app, mcp_tools: nil, rest_tools: nil)
  end

  def token(scopes)
    Doorkeeper::AccessToken.create!(application: @oauth_app, resource_owner_id: @user.id,
                                    token: SecureRandom.hex(32), scopes: scopes, expires_in: 3600)
  end

  def list_bots(with)
    get '/api/v1/bots', headers: { 'Authorization' => "Bearer #{with.token}" }
    JSON.parse(response.body).dig('error', 'code')
  end

  test 'follows the user REST switch both ways' do
    api = token('api')

    assert_equal 'tool_disabled', list_bots(api)

    @user.set_rest_tool_enabled('list_bots', true)
    assert_nil list_bots(api)
    assert_response :success

    @user.set_rest_tool_enabled('list_bots', false)
    assert_equal 'tool_disabled', list_bots(api)
  end

  test 'an mcp-only token reaches no REST tool even with the switch on' do
    @user.set_rest_tool_enabled('list_bots', true)

    assert_equal 'insufficient_scope', list_bots(token('mcp'))
    assert_response :forbidden
  end
end
