require 'test_helper'

class PerClientToolPermissionsTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @application = Doorkeeper::Application.create!(
      name: 'Test client', redirect_uri: 'http://localhost/callback',
      confidential: false, scopes: 'mcp'
    )
    ActionMCP::Current.stubs(:user).returns(@user)
  end

  test 'a granted tool runs' do
    ConnectedClient.create!(user: @user, oauth_application: @application, mcp_tools: %w[list_bots])
    OauthClientContext.oauth_application = @application

    response = ListBotsTool.call

    assert_not response.error?, response.contents.first&.text
  end

  test 'a tool the client was not granted is refused even though the user has it on' do
    ConnectedClient.create!(user: @user, oauth_application: @application, mcp_tools: %w[list_bots])
    OauthClientContext.oauth_application = @application

    assert @user.mcp_tool_enabled?('list_exchanges')
    response = ListExchangesTool.call

    assert response.error?
    assert_match(/not available to this client/i, response.contents.first.text)
  end

  test 'the user gate still reports as disabled, not as a client problem' do
    ConnectedClient.create!(user: @user, oauth_application: @application, mcp_tools: %w[list_bots])
    OauthClientContext.oauth_application = @application
    @user.set_mcp_tool_enabled('list_bots', false)

    response = ListBotsTool.call

    assert response.error?
    assert_match(/disabled/, response.contents.first.text)
  end

  test 'a call with no client identified is refused' do
    ConnectedClient.create!(user: @user, oauth_application: @application, mcp_tools: %w[list_bots])
    OauthClientContext.oauth_application = nil

    response = ListBotsTool.call

    assert response.error?
  end

  test 'the session registry carries the intersection, not the user set' do
    ConnectedClient.create!(
      user: @user, oauth_application: @application, mcp_tools: %w[list_bots market_buy]
    )
    OauthClientContext.oauth_application = @application

    session = Struct.new(:session_data, :tool_registry).new(nil, nil)
    gateway = ApplicationGateway.new(nil)
    gateway.stubs(:user).returns(@user)
    gateway.configure_session(session)

    # market_buy is granted but default-off for the user
    assert_equal %w[list_bots], session.tool_registry
    assert_equal({ 'user_id' => @user.id }, session.session_data)
  end

  test 'a client with no stored list lists and runs what the user has on, live' do
    ConnectedClient.create!(user: @user, oauth_application: @application, mcp_tools: nil, rest_tools: nil)
    OauthClientContext.oauth_application = @application
    session = Struct.new(:session_data, :tool_registry).new(nil, nil)
    gateway = ApplicationGateway.new(nil)
    gateway.stubs(:user).returns(@user)

    gateway.configure_session(session)
    assert_equal @user.enabled_mcp_tool_names.sort, session.tool_registry.sort
    assert_not ListBotsTool.call.error?

    @user.set_mcp_tool_enabled('list_bots', false)
    gateway.configure_session(session)
    assert_not_includes session.tool_registry, 'list_bots'
    assert_match(/disabled/, ListBotsTool.call.contents.first.text)
  end

  test 'an api-only token reaches no MCP tool even when the client follows the user' do
    ConnectedClient.create!(user: @user, oauth_application: @application, mcp_tools: nil, rest_tools: nil)
    token = Doorkeeper::AccessToken.create!(
      application: @application, resource_owner_id: @user.id,
      token: SecureRandom.hex(32), scopes: 'api', expires_in: 3600
    )
    request = Struct.new(:env).new({ 'HTTP_AUTHORIZATION' => "Bearer #{token.token}" })

    assert_raises(ActionMCP::GatewayIdentifier::Unauthorized) { MCPTokenIdentifier.new(request).resolve }
    assert_nil OauthClientContext.oauth_application
  end

  test 'the identifier publishes the client for the rest of the request' do
    token = Doorkeeper::AccessToken.create!(
      application: @application, resource_owner_id: @user.id,
      token: SecureRandom.hex(32), scopes: 'mcp', expires_in: 3600
    )
    env = { 'HTTP_AUTHORIZATION' => "Bearer #{token.token}" }
    request = Struct.new(:env).new(env)

    MCPTokenIdentifier.new(request).resolve

    assert_equal @application, OauthClientContext.oauth_application
  end

  test 'a failed resolution leaves no client behind' do
    OauthClientContext.oauth_application = @application
    env = { 'HTTP_AUTHORIZATION' => 'Bearer nope' }
    request = Struct.new(:env).new(env)

    assert_raises(ActionMCP::GatewayIdentifier::Unauthorized) do
      MCPTokenIdentifier.new(request).resolve
    end

    assert_nil OauthClientContext.oauth_application
  end

  test 'the task-augmented tool path stays off' do
    # ToolExecutionJob never rehydrates Current, so current_user and the client are
    # both nil inside a task-augmented tool. Enabling tasks without fixing that
    # would put an unauthenticated hole behind the gate.
    assert_not ActionMCP.configuration.tasks_enabled,
               'enabling tasks requires rehydrating the client in ToolExecutionJob first'
  end
end
