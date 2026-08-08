require 'test_helper'

class OauthConsentGrantTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    create(:user, admin: true, setup_completed: true)
    @user = create(:user, setup_completed: true)
    sign_in @user

    @application = Doorkeeper::Application.create!(
      name: 'Test client', redirect_uri: 'http://localhost/callback',
      confidential: false, scopes: 'mcp',
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

  test 'the consent screen offers a checkbox per tool group' do
    get oauth_authorization_path, params: authorize_params

    assert_response :success
    AppConfig::MCP_TOOL_GROUPS.each_key do |group|
      assert_select "input[type=checkbox][name='granted_mcp_groups[]'][value=#{group}]"
    end
  end

  test 'state-changing groups are not pre-checked' do
    get oauth_authorization_path, params: authorize_params

    assert_select "input[name='granted_mcp_groups[]'][value=read][checked]"
    assert_select "input[name='granted_mcp_groups[]'][value=trade][checked]", false,
                  'granting trading must be an affirmative act'
    assert_select "input[name='granted_mcp_groups[]'][value=control][checked]", false
  end

  test 'approving records the checked groups intersected with the user set' do
    assert_difference 'ConnectedClient.count', 1 do
      post oauth_authorization_path, params: authorize_params(granted_mcp_groups: %w[read])
    end

    client = ConnectedClient.last
    assert_equal @user, client.user
    assert_equal @application, client.oauth_application
    assert_includes client.granted_mcp_tools, 'list_bots'
    assert_not_includes client.granted_mcp_tools, 'download_tax_report'
  end

  test 'unchecking every group connects the client with nothing granted' do
    post oauth_authorization_path, params: authorize_params(granted_mcp_groups: [])

    assert_empty ConnectedClient.last.granted_mcp_tools
  end

  test 'denying records nothing' do
    assert_no_difference 'ConnectedClient.count' do
      delete oauth_authorization_path, params: authorize_params
    end
  end

  test 're-consent seeds the boxes from what the client already has' do
    ConnectedClient.create!(
      user: @user, oauth_application: @application,
      mcp_tools: AppConfig::MCP_TOOL_GROUPS['tax']
    )

    get oauth_authorization_path, params: authorize_params

    assert_select "input[name='granted_mcp_groups[]'][value=tax][checked]"
    assert_select "input[name='granted_mcp_groups[]'][value=read][checked]", false,
                  'a narrowed grant must not silently re-widen on re-consent'
  end

  test 'a partially granted group is not pre-ticked' do
    # Ticking a group grants the whole group. Pre-ticking a partial grant would
    # hand over the rest of it — including anything enabled globally since — on a
    # click the user reads as "yes, carry on".
    ConnectedClient.create!(
      user: @user, oauth_application: @application,
      mcp_tools: AppConfig::MCP_TOOL_GROUPS['read'].first(2)
    )

    get oauth_authorization_path, params: authorize_params

    assert_select "input[name='granted_mcp_groups[]'][value=read][checked]", false
  end

  test 'approving a partially granted group the user leaves unticked narrows it' do
    client = ConnectedClient.create!(
      user: @user, oauth_application: @application,
      mcp_tools: AppConfig::MCP_TOOL_GROUPS['read'].first(2)
    )

    post oauth_authorization_path, params: authorize_params(granted_mcp_groups: %w[tax])

    assert_empty client.reload.granted_mcp_tools & AppConfig::MCP_TOOL_GROUPS['read']
  end
end
