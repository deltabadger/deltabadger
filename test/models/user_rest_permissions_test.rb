# frozen_string_literal: true

require 'test_helper'

class UserRestPermissionsTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
  end

  # REST_TOOL_DEFAULTS — all off by default

  test 'rest_tool_enabled? returns false for read tools by default' do
    %w[list_bots get_bot_details list_exchanges get_exchange_balances
       get_portfolio_summary list_transactions list_open_orders
       export_transactions_csv list_account_transactions].each do |tool|
      assert_not @user.rest_tool_enabled?(tool), "Expected #{tool} to be disabled by default"
    end
  end

  test 'rest_tool_enabled? returns false for control tools by default' do
    %w[create_bot start_bot stop_bot update_bot_settings
       start_rule stop_rule update_rule_settings].each do |tool|
      assert_not @user.rest_tool_enabled?(tool), "Expected #{tool} to be disabled by default"
    end
  end

  test 'rest_tool_enabled? returns false for trade tools by default' do
    %w[market_buy market_sell limit_buy limit_sell cancel_order].each do |tool|
      assert_not @user.rest_tool_enabled?(tool), "Expected #{tool} to be disabled by default"
    end
  end

  test 'rest_tool_enabled? returns false for unknown tools' do
    assert_not @user.rest_tool_enabled?('nonexistent_tool')
  end

  test 'rest_tool_enabled? returns false for tax tools (out of REST scope)' do
    %w[list_tax_jurisdictions generate_tax_report get_tax_report_status
       download_tax_report].each do |tool|
      assert_not @user.rest_tool_enabled?(tool), "Expected #{tool} to be disabled (out of REST scope)"
    end
  end

  # set_rest_tool_enabled

  test 'set_rest_tool_enabled enables a tool' do
    @user.set_rest_tool_enabled('list_bots', true)
    assert @user.reload.rest_tool_enabled?('list_bots')
  end

  test 'set_rest_tool_enabled disables a tool that was enabled' do
    @user.set_rest_tool_enabled('list_bots', true)
    @user.set_rest_tool_enabled('list_bots', false)
    assert_not @user.reload.rest_tool_enabled?('list_bots')
  end

  test 'set_rest_tool_enabled preserves other tool settings' do
    @user.set_rest_tool_enabled('start_bot', true)
    @user.set_rest_tool_enabled('stop_bot', true)

    @user.reload
    assert @user.rest_tool_enabled?('start_bot')
    assert @user.rest_tool_enabled?('stop_bot')
    assert_not @user.rest_tool_enabled?('market_buy')
  end

  test 'REST permissions are independent per user' do
    user_b = create(:user)

    @user.set_rest_tool_enabled('market_buy', true)

    assert @user.reload.rest_tool_enabled?('market_buy')
    assert_not user_b.reload.rest_tool_enabled?('market_buy')
  end

  test 'REST permissions are independent from MCP permissions' do
    # Enabling MCP tool must NOT enable the REST tool of the same name.
    @user.set_mcp_tool_enabled('list_bots', true)
    @user.reload
    assert @user.mcp_tool_enabled?('list_bots')
    assert_not @user.rest_tool_enabled?('list_bots')

    # And vice versa: enabling REST must not flip MCP from its default.
    @user.set_rest_tool_enabled('start_bot', true)
    @user.reload
    assert @user.rest_tool_enabled?('start_bot')
    assert_not @user.mcp_tool_enabled?('start_bot') # MCP default for start_bot is false
  end

  # set_rest_tool_group_enabled

  test 'set_rest_tool_group_enabled enables all tools in trade group' do
    @user.set_rest_tool_group_enabled('trade', true)
    @user.reload

    %w[market_buy market_sell limit_buy limit_sell cancel_order].each do |tool|
      assert @user.rest_tool_enabled?(tool), "Expected #{tool} to be enabled"
    end
  end

  test 'set_rest_tool_group_enabled enables all tools in read group' do
    @user.set_rest_tool_group_enabled('read', true)
    @user.reload

    AppConfig::REST_TOOL_GROUPS['read'].each do |tool|
      assert @user.rest_tool_enabled?(tool), "Expected #{tool} to be enabled"
    end
  end

  test 'set_rest_tool_group_enabled disables all tools in a group' do
    @user.set_rest_tool_group_enabled('control', true)
    @user.set_rest_tool_group_enabled('control', false)
    @user.reload

    AppConfig::REST_TOOL_GROUPS['control'].each do |tool|
      assert_not @user.rest_tool_enabled?(tool), "Expected #{tool} to be disabled"
    end
  end

  test 'set_rest_tool_group_enabled is a no-op for unknown groups' do
    assert_nothing_raised do
      @user.set_rest_tool_group_enabled('not_a_real_group', true)
    end
    @user.reload
    # No tool should have been flipped on.
    assert_equal [], @user.enabled_rest_tool_names
  end

  test 'set_rest_tool_group_enabled does not touch other groups' do
    @user.set_rest_tool_group_enabled('read', true)
    @user.set_rest_tool_group_enabled('trade', true)
    @user.set_rest_tool_group_enabled('read', false)
    @user.reload

    AppConfig::REST_TOOL_GROUPS['read'].each do |tool|
      assert_not @user.rest_tool_enabled?(tool), "Expected #{tool} disabled after read group off"
    end
    %w[market_buy market_sell limit_buy limit_sell cancel_order].each do |tool|
      assert @user.rest_tool_enabled?(tool), "Expected #{tool} still enabled (trade group untouched)"
    end
  end

  # rest_tool_permissions / enabled_rest_tool_names

  test 'rest_tool_permissions returns hash of all REST tools with their status' do
    permissions = @user.rest_tool_permissions
    assert_instance_of Hash, permissions
    # Every key in REST_TOOL_DEFAULTS is represented; all default to false.
    assert_equal AppConfig::REST_TOOL_DEFAULTS.keys.sort, permissions.keys.sort
    permissions.each_value { |v| assert_equal false, v }
  end

  test 'rest_tool_permissions reflects overrides' do
    @user.set_rest_tool_enabled('list_bots', true)
    permissions = @user.reload.rest_tool_permissions
    assert_equal true, permissions['list_bots']
    assert_equal false, permissions['start_bot']
  end

  test 'enabled_rest_tool_names returns empty array by default' do
    assert_equal [], @user.enabled_rest_tool_names
  end

  test 'enabled_rest_tool_names returns only the enabled tools' do
    @user.set_rest_tool_enabled('list_bots', true)
    @user.set_rest_tool_enabled('market_buy', true)
    @user.reload

    assert_equal %w[list_bots market_buy].sort, @user.enabled_rest_tool_names.sort
  end

  # REST_TOOL_DEFAULTS / REST_TOOL_GROUPS structure

  test 'REST_TOOL_DEFAULTS contains the in-scope tool set (read/control/trade, no tax)' do
    expected = %w[
      list_bots get_bot_details list_exchanges get_exchange_balances
      get_portfolio_summary list_transactions list_open_orders
      export_transactions_csv list_account_transactions
      create_bot start_bot stop_bot update_bot_settings
      start_rule stop_rule update_rule_settings
      market_buy market_sell limit_buy limit_sell cancel_order
    ]
    assert_equal expected.sort, AppConfig::REST_TOOL_DEFAULTS.keys.sort
  end

  test 'REST_TOOL_DEFAULTS defaults every tool to false (opt-in)' do
    AppConfig::REST_TOOL_DEFAULTS.each do |tool, default|
      assert_equal false, default, "Expected #{tool} default to be false"
    end
  end

  test 'REST_TOOL_DEFAULTS keys are a subset of MCP_TOOL_DEFAULTS (identical names)' do
    extra = AppConfig::REST_TOOL_DEFAULTS.keys - AppConfig::MCP_TOOL_DEFAULTS.keys
    assert_empty extra, "REST tool keys must match existing MCP names; unexpected: #{extra.inspect}"
  end

  test 'REST_TOOL_DEFAULTS excludes tax-only tools' do
    excluded = %w[list_tax_jurisdictions generate_tax_report get_tax_report_status download_tax_report]
    excluded.each do |tool|
      assert_not AppConfig::REST_TOOL_DEFAULTS.key?(tool), "#{tool} must not be in REST scope"
    end
  end

  test 'REST_TOOL_GROUPS has read, control, and trade groups' do
    assert_equal %w[control read trade], AppConfig::REST_TOOL_GROUPS.keys.sort
  end

  test 'REST_TOOL_GROUPS has no tax group (out of REST scope)' do
    assert_not AppConfig::REST_TOOL_GROUPS.key?('tax')
  end

  test 'REST_TOOL_GROUPS values together cover exactly REST_TOOL_DEFAULTS keys' do
    grouped = AppConfig::REST_TOOL_GROUPS.values.flatten
    assert_equal grouped.sort, grouped.uniq.sort, 'a tool must appear in only one group'
    assert_equal AppConfig::REST_TOOL_DEFAULTS.keys.sort, grouped.sort
  end

  test "REST_TOOL_GROUPS['read'] includes activity exports moved out of MCP tax group" do
    assert_includes AppConfig::REST_TOOL_GROUPS['read'], 'export_transactions_csv'
    assert_includes AppConfig::REST_TOOL_GROUPS['read'], 'list_account_transactions'
  end

  test 'REST_TOOL_GROUPS trade group matches MCP trade group' do
    assert_equal AppConfig::MCP_TOOL_GROUPS['trade'].sort, AppConfig::REST_TOOL_GROUPS['trade'].sort
  end

  test 'REST_TOOL_GROUPS control group matches MCP control group' do
    assert_equal AppConfig::MCP_TOOL_GROUPS['control'].sort, AppConfig::REST_TOOL_GROUPS['control'].sort
  end
end
