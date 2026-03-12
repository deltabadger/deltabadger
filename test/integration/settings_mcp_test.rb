require 'test_helper'

class SettingsMcpTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = create(:user, admin: true, setup_completed: true)
    sign_in @admin
    @original_mcp_enabled = ENV['MCP_ENABLED']
    ENV['MCP_ENABLED'] = 'true'
  end

  teardown do
    AppConfig.clear_mcp_settings!
    ENV['MCP_ENABLED'] = @original_mcp_enabled
  end

  test 'mcp widget shows enable button when not configured' do
    get settings_path
    assert_response :success
    assert_select 'turbo-frame#mcp_settings'
  end

  test 'mcp widget shows URL when configured' do
    token = AppConfig.generate_mcp_access_token!
    get settings_path
    assert_response :success
    assert_select '#mcp_url_display', /#{token}/
  end

  test 'enable creates access token' do
    assert_not AppConfig.mcp_configured?

    patch settings_update_mcp_path
    assert_response :success

    assert AppConfig.mcp_configured?
    assert_match(/\A[a-f0-9]{32}\z/, AppConfig.mcp_access_token)
  end

  test 'revoke regenerates access token' do
    old_token = AppConfig.generate_mcp_access_token!

    delete settings_revoke_mcp_path
    assert_response :success

    assert AppConfig.mcp_configured?
    assert_not_equal old_token, AppConfig.mcp_access_token
  end

  test 'confirm revoke shows modal' do
    AppConfig.generate_mcp_access_token!

    get settings_confirm_revoke_mcp_path
    assert_response :success
  end

  test 'non-admin cannot enable' do
    regular_user = create(:user, setup_completed: true)
    sign_in regular_user

    patch settings_update_mcp_path
    assert_response :forbidden
  end

  test 'non-admin cannot revoke' do
    regular_user = create(:user, setup_completed: true)
    sign_in regular_user

    delete settings_revoke_mcp_path
    assert_response :forbidden
  end

  test 'mcp widget is hidden when MCP_ENABLED is not true' do
    ENV['MCP_ENABLED'] = nil

    get settings_path
    assert_response :success
    assert_select 'turbo-frame#mcp_settings', count: 0
  end

  test 'mcp widget is not shown to non-admin users' do
    regular_user = create(:user, setup_completed: true)
    sign_in regular_user

    get settings_path
    assert_response :success
    assert_select 'turbo-frame#mcp_settings', count: 0
  end
end
