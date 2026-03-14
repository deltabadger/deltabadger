require 'test_helper'

class SettingsMcpDryRunTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = create(:user, admin: true, setup_completed: true)
    sign_in @admin
    @original_mcp_enabled = ENV['MCP_ENABLED']
    ENV['MCP_ENABLED'] = 'true'
    AppConfig.generate_mcp_access_token!
  end

  teardown do
    AppConfig.clear_mcp_settings!
    ENV['MCP_ENABLED'] = @original_mcp_enabled
  end

  test 'toggle dry run on' do
    assert_not AppConfig.mcp_dry_run?

    patch settings_update_mcp_dry_run_path, params: { enabled: '1' }
    assert_response :success

    assert AppConfig.mcp_dry_run?
  end

  test 'toggle dry run off' do
    AppConfig.mcp_dry_run = true

    patch settings_update_mcp_dry_run_path, params: { enabled: '0' }
    assert_response :success

    assert_not AppConfig.mcp_dry_run?
  end

  test 'non-admin cannot toggle dry run' do
    regular_user = create(:user, setup_completed: true)
    sign_in regular_user

    patch settings_update_mcp_dry_run_path, params: { enabled: '1' }
    assert_response :forbidden
  end
end
