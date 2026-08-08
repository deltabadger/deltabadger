require 'test_helper'

class ToolAccessTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @application = Doorkeeper::Application.create!(
      name: 'Test client', redirect_uri: 'http://localhost/callback',
      confidential: false, scopes: 'mcp api'
    )
  end

  # --- the intersection ---------------------------------------------------

  test 'allows a tool the user has on and the client was granted' do
    ConnectedClient.create!(user: @user, oauth_application: @application, mcp_tools: %w[list_bots])

    assert ToolAccess.new(user: @user, application: @application).mcp_enabled?('list_bots')
  end

  test 'refuses a tool the user has on but the client was not granted' do
    ConnectedClient.create!(user: @user, oauth_application: @application, mcp_tools: %w[list_bots])

    assert @user.mcp_tool_enabled?('get_portfolio_summary')
    assert_not ToolAccess.new(user: @user, application: @application).mcp_enabled?('get_portfolio_summary')
  end

  test 'refuses a granted tool once the user turns it off globally' do
    ConnectedClient.create!(user: @user, oauth_application: @application, mcp_tools: %w[list_bots])
    @user.set_mcp_tool_enabled('list_bots', false)

    assert_not ToolAccess.new(user: @user, application: @application).mcp_enabled?('list_bots')
  end

  test 'a grant does not widen when the user enables a new tool later' do
    ConnectedClient.create!(user: @user, oauth_application: @application, mcp_tools: %w[list_bots])
    @user.set_mcp_tool_enabled('market_buy', true)

    assert_not ToolAccess.new(user: @user, application: @application).mcp_enabled?('market_buy')
  end

  test 'two clients of the same user are independent' do
    other_application = Doorkeeper::Application.create!(
      name: 'Other client', redirect_uri: 'http://localhost/other', confidential: false, scopes: 'mcp'
    )
    ConnectedClient.create!(user: @user, oauth_application: @application, mcp_tools: %w[list_bots])
    ConnectedClient.create!(user: @user, oauth_application: other_application, mcp_tools: %w[list_exchanges])

    assert ToolAccess.new(user: @user, application: @application).mcp_enabled?('list_bots')
    assert_not ToolAccess.new(user: @user, application: other_application).mcp_enabled?('list_bots')
  end

  # --- fail closed --------------------------------------------------------

  test 'refuses everything when the client has no grant record' do
    access = ToolAccess.new(user: @user, application: @application)

    assert_not access.mcp_enabled?('list_bots')
    assert_not access.rest_enabled?('list_bots')
    assert_empty access.enabled_mcp_tool_names
  end

  test 'refuses everything when there is no client at all' do
    assert_not ToolAccess.new(user: @user, application: nil).mcp_enabled?('list_bots')
    assert_empty ToolAccess.new(user: @user, application: nil).enabled_mcp_tool_names
  end

  test 'refuses everything when there is no user' do
    assert_not ToolAccess.new(user: nil, application: @application).mcp_enabled?('list_bots')
    assert_empty ToolAccess.new(user: nil, application: nil).enabled_mcp_tool_names
  end

  # --- the personal access token ------------------------------------------

  test 'a personal access token application is the user acting as themselves' do
    @user.set_rest_tool_enabled('list_bots', true)
    personal = @user.personal_api_token.application

    access = ToolAccess.new(user: @user, application: personal)

    assert personal.personal_access_token?
    assert access.rest_enabled?('list_bots')
    assert_not access.rest_enabled?('market_buy'), 'still bounded by the user setting'
  end

  test 'the personal application needs no grant record' do
    personal = @user.personal_api_token.application

    assert_nil ConnectedClient.for(user: @user, application: personal)
    assert_not_empty ToolAccess.new(user: @user, application: personal).enabled_mcp_tool_names
  end

  test 'another user personal application grants nothing' do
    other = create(:user)
    other.set_rest_tool_enabled('list_bots', true)
    @user.set_rest_tool_enabled('list_bots', true)
    theirs = other.personal_api_token.application

    access = ToolAccess.new(user: @user, application: theirs)

    assert theirs.personal_access_token?
    assert_not access.rest_enabled?('list_bots'), 'the exemption is not transferable'
    assert_empty access.enabled_mcp_tool_names
  end

  # --- REST ---------------------------------------------------------------

  test 'rest tools intersect the same way' do
    ConnectedClient.create!(user: @user, oauth_application: @application, rest_tools: %w[list_bots market_buy])
    @user.set_rest_tool_enabled('list_bots', true)

    access = ToolAccess.new(user: @user, application: @application)

    assert access.rest_enabled?('list_bots')
    assert_not access.rest_enabled?('market_buy'), 'granted but off for the user'
  end

  test 'mcp and rest grants do not leak into each other' do
    ConnectedClient.create!(user: @user, oauth_application: @application, mcp_tools: %w[list_bots])
    @user.set_rest_tool_enabled('list_bots', true)

    access = ToolAccess.new(user: @user, application: @application)

    assert access.mcp_enabled?('list_bots')
    assert_not access.rest_enabled?('list_bots')
  end

  # --- the registry list --------------------------------------------------

  test 'enabled_mcp_tool_names is the intersection' do
    ConnectedClient.create!(user: @user, oauth_application: @application, mcp_tools: %w[list_bots market_buy])

    assert_equal %w[list_bots], ToolAccess.new(user: @user, application: @application).enabled_mcp_tool_names
  end

  test 'unknown tool names are refused' do
    ConnectedClient.create!(user: @user, oauth_application: @application, mcp_tools: %w[renamed_away])

    access = ToolAccess.new(user: @user, application: @application)

    assert_not access.mcp_enabled?('renamed_away')
    assert_empty access.enabled_mcp_tool_names
  end
end
