# frozen_string_literal: true

require 'test_helper'

class SettingsMcpToolPermissionsTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = create(:user, admin: true, setup_completed: true)
    sign_in @admin
  end

  test 'mcp widget shows tool toggles' do
    get settings_connect_path
    assert_response :success
    assert_select '#mcp_tool_permissions'
  end

  test 'update_mcp_tool_permissions enables a tool' do
    patch settings_update_mcp_tool_permissions_path, params: { tool_name: 'start_bot', enabled: '1' }
    assert_response :success
    assert @admin.reload.mcp_tool_enabled?('start_bot')
  end

  test 'update_mcp_tool_permissions disables a tool' do
    @admin.set_mcp_tool_enabled('start_bot', true)
    patch settings_update_mcp_tool_permissions_path, params: { tool_name: 'start_bot', enabled: '0' }
    assert_response :success
    assert_not @admin.reload.mcp_tool_enabled?('start_bot')
  end

  test 'rejects unknown tool names' do
    patch settings_update_mcp_tool_permissions_path, params: { tool_name: 'hack_the_planet', enabled: '1' }
    assert_response :unprocessable_entity
  end

  test 'non-admin can update their own tool permissions' do
    regular_user = create(:user, setup_completed: true)
    sign_in regular_user

    patch settings_update_mcp_tool_permissions_path, params: { tool_name: 'start_bot', enabled: '1' }
    assert_response :success
    assert regular_user.reload.mcp_tool_enabled?('start_bot')
  end

  test 'update_mcp_tool_group_permissions enables all tools in a group' do
    patch settings_update_mcp_tool_group_permissions_path, params: { group: 'trade', enabled: '1' }
    assert_response :success
    @admin.reload
    %w[market_buy market_sell limit_buy limit_sell cancel_order].each do |tool|
      assert @admin.mcp_tool_enabled?(tool), "Expected #{tool} to be enabled"
    end
  end

  test 'update_mcp_tool_group_permissions disables all tools in a group' do
    AppConfig::MCP_TOOL_GROUPS['read'].each { |tool| @admin.set_mcp_tool_enabled(tool, true) }
    patch settings_update_mcp_tool_group_permissions_path, params: { group: 'read', enabled: '0' }
    assert_response :success
    @admin.reload
    %w[list_bots get_bot_details list_exchanges get_exchange_balances get_portfolio_summary list_transactions list_open_orders].each do |tool|
      assert_not @admin.mcp_tool_enabled?(tool), "Expected #{tool} to be disabled"
    end
  end

  test 'rejects unknown group names' do
    patch settings_update_mcp_tool_group_permissions_path, params: { group: 'unknown', enabled: '1' }
    assert_response :unprocessable_entity
  end
end
