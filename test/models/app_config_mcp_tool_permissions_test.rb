# frozen_string_literal: true

require 'test_helper'

class AppConfigMcpToolPermissionsTest < ActiveSupport::TestCase
  teardown do
    AppConfig.delete(AppConfig::MCP_TOOL_PERMISSIONS)
  end

  test 'mcp_tool_enabled? returns true for read-only tools by default' do
    assert AppConfig.mcp_tool_enabled?('list_bots')
    assert AppConfig.mcp_tool_enabled?('get_bot_details')
    assert AppConfig.mcp_tool_enabled?('list_exchanges')
    assert AppConfig.mcp_tool_enabled?('get_exchange_balances')
    assert AppConfig.mcp_tool_enabled?('get_portfolio_summary')
    assert AppConfig.mcp_tool_enabled?('list_transactions')
  end

  test 'mcp_tool_enabled? returns false for write tools by default' do
    assert_not AppConfig.mcp_tool_enabled?('start_bot')
    assert_not AppConfig.mcp_tool_enabled?('stop_bot')
    assert_not AppConfig.mcp_tool_enabled?('update_bot_settings')
    assert_not AppConfig.mcp_tool_enabled?('start_rule')
    assert_not AppConfig.mcp_tool_enabled?('stop_rule')
    assert_not AppConfig.mcp_tool_enabled?('market_buy')
    assert_not AppConfig.mcp_tool_enabled?('market_sell')
    assert_not AppConfig.mcp_tool_enabled?('limit_buy')
    assert_not AppConfig.mcp_tool_enabled?('limit_sell')
  end

  test 'set_mcp_tool_enabled enables a tool' do
    AppConfig.set_mcp_tool_enabled('start_bot', true)
    assert AppConfig.mcp_tool_enabled?('start_bot')
  end

  test 'set_mcp_tool_enabled disables a tool' do
    AppConfig.set_mcp_tool_enabled('list_bots', false)
    assert_not AppConfig.mcp_tool_enabled?('list_bots')
  end

  test 'mcp_tool_permissions returns hash of all tools with their status' do
    permissions = AppConfig.mcp_tool_permissions
    assert_instance_of Hash, permissions
    assert_equal true, permissions['list_bots']
    assert_equal false, permissions['start_bot']
  end

  test 'set_mcp_tool_enabled preserves other tool settings' do
    AppConfig.set_mcp_tool_enabled('start_bot', true)
    AppConfig.set_mcp_tool_enabled('stop_bot', true)

    assert AppConfig.mcp_tool_enabled?('start_bot')
    assert AppConfig.mcp_tool_enabled?('stop_bot')
    assert_not AppConfig.mcp_tool_enabled?('market_buy')
  end

  test 'clear_mcp_settings also clears tool permissions' do
    AppConfig.set_mcp_tool_enabled('start_bot', true)
    AppConfig.clear_mcp_settings!
    assert_not AppConfig.mcp_tool_enabled?('start_bot')
  end

  test 'mcp_tool_enabled? returns false for unknown tools' do
    assert_not AppConfig.mcp_tool_enabled?('nonexistent_tool')
  end

  test 'MCP_TOOL_DEFAULTS contains all expected tools' do
    expected = %w[
      list_bots get_bot_details list_exchanges get_exchange_balances
      get_portfolio_summary list_transactions
      start_bot stop_bot update_bot_settings
      start_rule stop_rule
      market_buy market_sell
      limit_buy limit_sell
    ]
    assert_equal expected.sort, AppConfig::MCP_TOOL_DEFAULTS.keys.sort
  end
end
