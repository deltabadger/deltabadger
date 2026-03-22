# frozen_string_literal: true

require 'test_helper'

class UserMcpPermissionsTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
  end

  test 'mcp_tool_enabled? returns true for read-only tools by default' do
    assert @user.mcp_tool_enabled?('list_bots')
    assert @user.mcp_tool_enabled?('get_bot_details')
    assert @user.mcp_tool_enabled?('list_exchanges')
    assert @user.mcp_tool_enabled?('get_exchange_balances')
    assert @user.mcp_tool_enabled?('get_portfolio_summary')
    assert @user.mcp_tool_enabled?('list_transactions')
  end

  test 'mcp_tool_enabled? returns false for write tools by default' do
    assert_not @user.mcp_tool_enabled?('start_bot')
    assert_not @user.mcp_tool_enabled?('stop_bot')
    assert_not @user.mcp_tool_enabled?('update_bot_settings')
    assert_not @user.mcp_tool_enabled?('start_rule')
    assert_not @user.mcp_tool_enabled?('stop_rule')
    assert_not @user.mcp_tool_enabled?('market_buy')
    assert_not @user.mcp_tool_enabled?('market_sell')
    assert_not @user.mcp_tool_enabled?('limit_buy')
    assert_not @user.mcp_tool_enabled?('limit_sell')
  end

  test 'set_mcp_tool_enabled enables a tool' do
    @user.set_mcp_tool_enabled('start_bot', true)
    assert @user.reload.mcp_tool_enabled?('start_bot')
  end

  test 'set_mcp_tool_enabled disables a tool' do
    @user.set_mcp_tool_enabled('list_bots', false)
    assert_not @user.reload.mcp_tool_enabled?('list_bots')
  end

  test 'mcp_tool_permissions returns hash of all tools with their status' do
    permissions = @user.mcp_tool_permissions
    assert_instance_of Hash, permissions
    assert_equal true, permissions['list_bots']
    assert_equal false, permissions['start_bot']
  end

  test 'set_mcp_tool_enabled preserves other tool settings' do
    @user.set_mcp_tool_enabled('start_bot', true)
    @user.set_mcp_tool_enabled('stop_bot', true)

    @user.reload
    assert @user.mcp_tool_enabled?('start_bot')
    assert @user.mcp_tool_enabled?('stop_bot')
    assert_not @user.mcp_tool_enabled?('market_buy')
  end

  test 'mcp_tool_enabled? returns false for unknown tools' do
    assert_not @user.mcp_tool_enabled?('nonexistent_tool')
  end

  test 'MCP_TOOL_DEFAULTS contains all expected tools' do
    expected = %w[
      list_bots get_bot_details list_exchanges get_exchange_balances
      get_portfolio_summary list_transactions list_open_orders
      create_bot start_bot stop_bot update_bot_settings
      start_rule stop_rule update_rule_settings
      market_buy market_sell
      limit_buy limit_sell cancel_order
    ]
    assert_equal expected.sort, AppConfig::MCP_TOOL_DEFAULTS.keys.sort
  end
end
