# frozen_string_literal: true

require 'test_helper'

class SettingsMcpToolPermissionsTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = create(:user, admin: true, setup_completed: true)
    sign_in @admin
  end

  # ---- the matrix ---------------------------------------------------------
  #
  # The MCP section's tool list: a row per tool, under a row per group, each with its switch.

  test 'the permissions frame renders one row per MCP tool with an MCP switch' do
    get settings_mcp_path
    assert_response :success
    assert_select 'turbo-frame#tool_permissions' do
      assert_select '.srow--tool', AppConfig::MCP_TOOL_DEFAULTS.size
      AppConfig::MCP_TOOL_DEFAULTS.each_key do |tool|
        assert_select ".srow--tool[data-tool=#{tool}][data-surface=mcp] form[action=?]",
                      settings_update_mcp_tool_permissions_path, 1 do
          assert_select 'input[name=tool_name][value=?]', tool
        end
      end
    end
  end

  test 'tool rows show the tool description next to its name' do
    get settings_mcp_path
    assert_select 'turbo-frame#tool_permissions .srow--tool[data-tool=market_buy]' do
      assert_select '.srow__label', I18n.t('settings.mcp.tools.market_buy.label')
      assert_select '.srow__hint', I18n.t('settings.mcp.tools.market_buy.description')
    end
  end

  test 'a tool switch posts the opposite of its current state' do
    @admin.set_mcp_tool_enabled('start_bot', true)
    get settings_mcp_path

    assert_select 'turbo-frame#tool_permissions .srow--tool[data-tool=start_bot][data-surface=mcp] form' do
      assert_select 'input[name=enabled][value=0]', 1
      assert_select 'input[type=checkbox][checked]', 1
    end
    assert_select 'turbo-frame#tool_permissions .srow--tool[data-tool=stop_bot][data-surface=mcp] form' do
      assert_select 'input[name=enabled][value=1]', 1
      assert_select 'input[type=checkbox]:not([checked])', 1
    end
  end

  test 'each group row carries a switch that flips the whole MCP group' do
    get settings_mcp_path
    assert_select 'turbo-frame#tool_permissions .srow--group', AppConfig::MCP_TOOL_GROUPS.size
    AppConfig::MCP_TOOL_GROUPS.each_key do |group|
      assert_select "turbo-frame#tool_permissions .srow--group[data-group=#{group}][data-surface=mcp] form[action=?]",
                    settings_update_mcp_tool_group_permissions_path, 1 do
        assert_select 'input[name=group][value=?]', group
      end
    end
  end

  test 'a group with some of its tools on renders as partial, and a click turns the rest on' do
    AppConfig::MCP_TOOL_GROUPS['control'].each { |t| @admin.set_mcp_tool_enabled(t, false) }
    @admin.set_mcp_tool_enabled('start_bot', true)
    get settings_mcp_path

    assert_select 'turbo-frame#tool_permissions .srow--group[data-group=control][data-surface=mcp]' do
      assert_select '.toggle.toggle--partial input[type=checkbox][checked]', 1
      assert_select 'input[name=enabled][value=1]', 1
    end
  end

  test 'a group with every tool on is a plain checked switch that turns the group off' do
    AppConfig::MCP_TOOL_GROUPS['trade'].each { |t| @admin.set_mcp_tool_enabled(t, true) }
    get settings_mcp_path

    assert_select 'turbo-frame#tool_permissions .srow--group[data-group=trade][data-surface=mcp]' do
      assert_select '.toggle:not(.toggle--partial) input[type=checkbox][checked]', 1
      assert_select 'input[name=enabled][value=0]', 1
    end
  end

  # The client cards derive ON/PARTIAL from the user's own set, so they re-render with the matrix;
  # the mcp widget holds nothing that changes and stays put.
  test 'a tool switch re-renders the permissions and the clients, not the mcp widget' do
    patch settings_update_mcp_tool_permissions_path, params: { tool_name: 'start_bot', enabled: '1' }
    assert_response :success
    assert_select 'turbo-stream[action=replace][target=tool_permissions]', 1
    assert_select 'turbo-stream[action=replace][target=connected_clients]', 1
    assert_select 'turbo-stream[target=mcp_settings]', 0
  end

  test 'a group switch re-renders the permissions and the clients' do
    patch settings_update_mcp_tool_group_permissions_path, params: { group: 'trade', enabled: '1' }
    assert_response :success
    assert_select 'turbo-stream[action=replace][target=tool_permissions]', 1
    assert_select 'turbo-stream[action=replace][target=connected_clients]', 1
  end

  # ---- the actions --------------------------------------------------------

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
