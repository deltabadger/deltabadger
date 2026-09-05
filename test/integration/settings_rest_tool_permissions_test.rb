# frozen_string_literal: true

require 'test_helper'

class SettingsRestToolPermissionsTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = create(:user, admin: true, setup_completed: true)
    sign_in @admin
  end

  # ---- widget render ------------------------------------------------------

  # REST shares the tool matrix with MCP: its switches live in the REST column of the same
  # table, one per REST tool plus one per REST group, and nowhere else.
  test 'the REST column carries one switch per REST tool and one per REST group' do
    get settings_api_path
    assert_response :success
    assert_select 'turbo-frame#tool_permissions td[data-surface=rest] form',
                  AppConfig::REST_TOOL_DEFAULTS.size + AppConfig::REST_TOOL_GROUPS.size
    AppConfig::REST_TOOL_DEFAULTS.each_key do |tool|
      assert_select "tr.permissions__tool[data-tool=#{tool}] td[data-surface=rest] form[action=?]",
                    settings_update_rest_tool_permissions_path, 1
    end
    AppConfig::REST_TOOL_GROUPS.each_key do |group|
      assert_select "tr.permissions__group[data-group=#{group}] td[data-surface=rest] form[action=?]",
                    settings_update_rest_tool_group_permissions_path, 1
    end
    assert_select 'turbo-frame#rest_settings input[name=tool_name]', 0
  end

  # The rows follow the MCP grouping. REST files the two export tools under Read and has no Tax
  # group, so the Tax section shows REST switches for those two rows and none for the rest of it.
  test 'tools outside REST scope have an empty REST cell, not a switch' do
    get settings_api_path
    (AppConfig::MCP_TOOL_DEFAULTS.keys - AppConfig::REST_TOOL_DEFAULTS.keys).each do |tool|
      assert_select "tr.permissions__tool[data-tool=#{tool}] td[data-surface=rest]", 1 do
        assert_select 'form', 0
      end
    end
    assert_select 'tr.permissions__group[data-group=tax] td[data-surface=rest]', 1 do
      assert_select 'form', 0
    end
    assert_select 'tr.permissions__tool[data-tool=export_transactions_csv] td[data-surface=rest] form', 1
  end

  test 'REST widget renders alongside the MCP widget (both present on the page)' do
    get settings_api_path
    assert_response :success
    assert_select 'turbo-frame#mcp_settings'
    assert_select 'turbo-frame#rest_settings'
  end

  test 'a REST tool switch re-renders the permissions and the clients, not the REST widget' do
    patch settings_update_rest_tool_permissions_path, params: { tool_name: 'list_bots', enabled: '1' }
    assert_response :success
    assert_select 'turbo-stream[action=replace][target=tool_permissions]', 1
    assert_select 'turbo-stream[action=replace][target=connected_clients]', 1
    assert_select 'turbo-stream[target=rest_settings]', 0
  end

  test 'REST widget exposes a Download API docs link' do
    get settings_api_path
    assert_response :success
    assert_select 'a[href=?]', settings_download_api_docs_path
  end

  test 'GET /settings/download_api_docs returns docs/api.md as a markdown attachment' do
    get settings_download_api_docs_path

    assert_response :success
    assert_match(%r{text/markdown}, response.headers['Content-Type'])
    assert_match(/attachment; filename="deltabadger-api.md"/, response.headers['Content-Disposition'])
    # Sanity-check that the served content is the docs file, not a stub.
    assert_match(/Deltabadger REST API/, response.body)
    assert_match(/Idempotency-Key/, response.body)
  end

  # ---- per-tool toggle ----------------------------------------------------

  test 'update_rest_tool_permissions enables a tool' do
    patch settings_update_rest_tool_permissions_path, params: { tool_name: 'list_bots', enabled: '1' }
    assert_response :success
    assert @admin.reload.rest_tool_enabled?('list_bots')
  end

  test 'update_rest_tool_permissions disables a tool' do
    @admin.set_rest_tool_enabled('list_bots', true)
    patch settings_update_rest_tool_permissions_path, params: { tool_name: 'list_bots', enabled: '0' }
    assert_response :success
    assert_not @admin.reload.rest_tool_enabled?('list_bots')
  end

  test 'rejects unknown REST tool names' do
    patch settings_update_rest_tool_permissions_path, params: { tool_name: 'hack_the_planet', enabled: '1' }
    assert_response :unprocessable_entity
  end

  test 'rejects tax-scoped tool names (not in REST scope)' do
    # generate_tax_report is a valid MCP tool but is intentionally excluded
    # from REST. It must be rejected here, not silently accepted.
    patch settings_update_rest_tool_permissions_path, params: { tool_name: 'generate_tax_report', enabled: '1' }
    assert_response :unprocessable_entity
  end

  test 'non-admin can manage their own REST tool permissions' do
    regular_user = create(:user, setup_completed: true)
    sign_in regular_user

    patch settings_update_rest_tool_permissions_path, params: { tool_name: 'list_bots', enabled: '1' }
    assert_response :success
    assert regular_user.reload.rest_tool_enabled?('list_bots')
  end

  # ---- group toggle -------------------------------------------------------

  test 'update_rest_tool_group_permissions enables every tool in a group' do
    patch settings_update_rest_tool_group_permissions_path, params: { group: 'trade', enabled: '1' }
    assert_response :success
    @admin.reload
    %w[market_buy market_sell limit_buy limit_sell cancel_order].each do |tool|
      assert @admin.rest_tool_enabled?(tool), "Expected #{tool} enabled"
    end
  end

  test 'update_rest_tool_group_permissions disables every tool in a group' do
    AppConfig::REST_TOOL_GROUPS['read'].each { |t| @admin.set_rest_tool_enabled(t, true) }
    patch settings_update_rest_tool_group_permissions_path, params: { group: 'read', enabled: '0' }
    assert_response :success
    @admin.reload
    AppConfig::REST_TOOL_GROUPS['read'].each do |tool|
      assert_not @admin.rest_tool_enabled?(tool), "Expected #{tool} disabled"
    end
  end

  test 'rejects unknown REST group names' do
    patch settings_update_rest_tool_group_permissions_path, params: { group: 'tax', enabled: '1' }
    assert_response :unprocessable_entity
  end

  # ---- isolation from MCP -------------------------------------------------

  test 'enabling a REST tool does not touch its MCP twin' do
    # MCP default for start_bot is false. Enabling it on REST must leave MCP alone.
    patch settings_update_rest_tool_permissions_path, params: { tool_name: 'start_bot', enabled: '1' }
    assert_response :success
    @admin.reload
    assert @admin.rest_tool_enabled?('start_bot')
    assert_not @admin.mcp_tool_enabled?('start_bot')
  end

  test 'enabling an MCP tool does not touch its REST twin' do
    patch settings_update_mcp_tool_permissions_path, params: { tool_name: 'list_bots', enabled: '1' }
    assert_response :success
    @admin.reload
    assert @admin.mcp_tool_enabled?('list_bots')
    assert_not @admin.rest_tool_enabled?('list_bots') # REST defaults are all-off
  end

  test 'disabling a REST group does not flip its MCP equivalent' do
    AppConfig::MCP_TOOL_GROUPS['read'].each { |t| @admin.set_mcp_tool_enabled(t, true) }
    AppConfig::REST_TOOL_GROUPS['read'].each { |t| @admin.set_rest_tool_enabled(t, true) }

    patch settings_update_rest_tool_group_permissions_path, params: { group: 'read', enabled: '0' }
    assert_response :success
    @admin.reload

    AppConfig::REST_TOOL_GROUPS['read'].each { |t| assert_not @admin.rest_tool_enabled?(t), "#{t} should be REST-off" }
    AppConfig::MCP_TOOL_GROUPS['read'].each { |t| assert @admin.mcp_tool_enabled?(t), "#{t} should stay MCP-on" }
  end
end
