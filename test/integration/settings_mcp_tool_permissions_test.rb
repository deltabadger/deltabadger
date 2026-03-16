# frozen_string_literal: true

require 'test_helper'

class SettingsMcpToolPermissionsTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = create(:user, admin: true, setup_completed: true)
    sign_in @admin
    @original_mcp_enabled = ENV['MCP_ENABLED']
    ENV['MCP_ENABLED'] = 'true'
  end

  teardown do
    AppConfig.clear_mcp_settings!
    AppConfig.delete(AppConfig::MCP_TOOL_PERMISSIONS)
    ENV['MCP_ENABLED'] = @original_mcp_enabled
  end

  test 'mcp widget shows tool toggles' do
    get settings_path
    assert_response :success
    assert_select '#mcp_tool_permissions'
  end

  test 'update_mcp_tool_permissions enables a tool' do
    patch settings_update_mcp_tool_permissions_path, params: { tool_name: 'start_bot', enabled: '1' }
    assert_response :success
    assert AppConfig.mcp_tool_enabled?('start_bot')
  end

  test 'update_mcp_tool_permissions disables a tool' do
    AppConfig.set_mcp_tool_enabled('start_bot', true)
    patch settings_update_mcp_tool_permissions_path, params: { tool_name: 'start_bot', enabled: '0' }
    assert_response :success
    assert_not AppConfig.mcp_tool_enabled?('start_bot')
  end

  test 'rejects unknown tool names' do
    patch settings_update_mcp_tool_permissions_path, params: { tool_name: 'hack_the_planet', enabled: '1' }
    assert_response :unprocessable_entity
  end

  test 'non-admin cannot update tool permissions' do
    regular_user = create(:user, setup_completed: true)
    sign_in regular_user

    patch settings_update_mcp_tool_permissions_path, params: { tool_name: 'start_bot', enabled: '1' }
    assert_response :forbidden
  end

  test 'update_mcp_tool_group_permissions enables all tools in a group' do
    patch settings_update_mcp_tool_group_permissions_path, params: { group: 'trade', enabled: '1' }
    assert_response :success
    %w[market_buy market_sell limit_buy limit_sell cancel_order].each do |tool|
      assert AppConfig.mcp_tool_enabled?(tool), "Expected #{tool} to be enabled"
    end
  end

  test 'update_mcp_tool_group_permissions disables all tools in a group' do
    AppConfig::MCP_TOOL_GROUPS['read'].each { |tool| AppConfig.set_mcp_tool_enabled(tool, true) }
    patch settings_update_mcp_tool_group_permissions_path, params: { group: 'read', enabled: '0' }
    assert_response :success
    %w[list_bots get_bot_details list_exchanges get_exchange_balances get_portfolio_summary list_transactions list_open_orders].each do |tool|
      assert_not AppConfig.mcp_tool_enabled?(tool), "Expected #{tool} to be disabled"
    end
  end

  test 'rejects unknown group names' do
    patch settings_update_mcp_tool_group_permissions_path, params: { group: 'unknown', enabled: '1' }
    assert_response :unprocessable_entity
  end
end
