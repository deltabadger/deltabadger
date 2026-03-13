# frozen_string_literal: true

require 'test_helper'

class StartRuleToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @exchange = create(:binance_exchange)
    @asset = create(:asset, :bitcoin)
    @rule = Rules::Withdrawal.create!(
      user: @user,
      exchange: @exchange,
      asset: @asset,
      address: '0xabc123',
      status: :stopped,
      settings: { 'max_fee_percentage' => '5', 'threshold_type' => 'max_fee_percentage' }
    )
    ActionMCP::Current.user = @user
    AppConfig.set_mcp_tool_enabled('start_rule', true)
  end

  teardown do
    ActionMCP::Current.reset
    AppConfig.delete(AppConfig::MCP_TOOL_PERMISSIONS)
  end

  test 'starts a stopped rule' do
    response = StartRuleTool.new(rule_id: @rule.id).execute

    @rule.reload
    assert @rule.scheduled?
    assert_match(/started/, response.contents.first.text)
  end

  test 'returns error for already active rule' do
    @rule.update!(status: :scheduled)

    response = StartRuleTool.new(rule_id: @rule.id).execute

    assert_match(/already active/, response.contents.first.text)
  end

  test 'returns error for non-existent rule' do
    response = StartRuleTool.new(rule_id: 999_999).execute

    assert_match(/not found/, response.contents.first.text)
  end

  test 'returns error when tool is disabled' do
    AppConfig.set_mcp_tool_enabled('start_rule', false)

    response = StartRuleTool.new(rule_id: @rule.id).execute

    assert_match(/disabled/, response.contents.first.text)
    @rule.reload
    assert @rule.stopped?
  end
end
