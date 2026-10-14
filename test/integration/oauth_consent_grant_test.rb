require 'test_helper'

class OauthConsentGrantTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    create(:user, admin: true, setup_completed: true)
    @user = create(:user, setup_completed: true)
    sign_in @user

    @application = Doorkeeper::Application.create!(
      name: 'Test client', redirect_uri: 'http://localhost/callback',
      confidential: false, scopes: 'mcp api',
      token_endpoint_auth_method: 'none', grant_types: 'authorization_code',
      response_types: 'code'
    )
  end

  def authorize_params(extra = {})
    {
      client_id: @application.uid,
      redirect_uri: 'http://localhost/callback',
      response_type: 'code',
      scope: 'mcp',
      code_challenge: 'E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM',
      code_challenge_method: 'S256'
    }.merge(extra)
  end

  def forged_groups = { granted_mcp_groups: %w[read control trade tax], granted_rest_groups: %w[read] }

  def stored(client) = client.reload.attributes.slice('mcp_tools', 'rest_tools', 'updated_at')

  test 'the consent screen has no checkboxes' do
    get oauth_authorization_path, params: authorize_params

    assert_response :success
    assert_select 'input[type=checkbox]', false
  end

  test 'a new connection stores no tool list and follows the user' do
    assert_difference 'ConnectedClient.count', 1 do
      post oauth_authorization_path, params: authorize_params
    end

    client = ConnectedClient.last
    assert_equal [@user, @application], [client.user, client.oauth_application]
    assert_nil client.mcp_tools
    assert_nil client.rest_tools
    assert_equal @user.enabled_mcp_tool_names.sort, client.granted_mcp_tools.sort
  end

  test 'groups forged into the form or the query are ignored on a new connection' do
    post oauth_authorization_path, params: authorize_params(forged_groups)
    assert_nil ConnectedClient.last.mcp_tools
    ConnectedClient.delete_all

    post "#{oauth_authorization_path}?#{forged_groups.to_query}", params: authorize_params
    assert_nil ConnectedClient.last.mcp_tools
    assert_nil ConnectedClient.last.rest_tools
  end

  test 'denying records nothing' do
    assert_no_difference 'ConnectedClient.count' do
      delete oauth_authorization_path, params: authorize_params
    end
  end

  # Whatever the user set for this client in Settings stands: explicit, empty, mixed or
  # following. Approving again, even with forged groups, writes nothing.
  {
    'explicit' => { mcp_tools: %w[list_bots], rest_tools: %w[list_bots] },
    'empty' => { mcp_tools: [], rest_tools: [] },
    'mixed' => { mcp_tools: %w[list_bots], rest_tools: nil },
    'following' => { mcp_tools: nil, rest_tools: nil }
  }.each do |name, columns|
    test "re-consent leaves a #{name} grant exactly as it was" do
      client = ConnectedClient.create!(user: @user, oauth_application: @application, **columns)
      client.update_columns(updated_at: 1.day.ago)
      before = stored(client)

      assert_no_difference 'ConnectedClient.count' do
        post oauth_authorization_path, params: authorize_params(scope: 'mcp api')
        post "#{oauth_authorization_path}?#{forged_groups.to_query}", params: authorize_params(forged_groups)
      end

      assert_equal before, stored(client)
    end
  end
end
