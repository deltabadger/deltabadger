# frozen_string_literal: true

require 'test_helper'

class UpdateRuleSettingsToolTest < ActiveSupport::TestCase
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
    @user.set_mcp_tool_enabled('update_rule_settings', true)
  end

  teardown do
    ActionMCP::Current.reset
  end

  test 'updates rule max_fee_percentage' do
    response = UpdateRuleSettingsTool.new(rule_id: @rule.id, max_fee_percentage: 1.5).execute

    assert_match(/updated/, response.contents.first.text)
    assert_equal '1.5', @rule.reload.max_fee_percentage
  end

  test 'updates rule withdrawal_percentage' do
    response = UpdateRuleSettingsTool.new(rule_id: @rule.id, withdrawal_percentage: 80).execute

    assert_match(/updated/, response.contents.first.text)
    assert_equal '80.0', @rule.reload.withdrawal_percentage
  end

  test 'updates rule threshold_type' do
    response = UpdateRuleSettingsTool.new(rule_id: @rule.id, threshold_type: 'min_amount', min_amount: 10).execute

    assert_match(/updated/, response.contents.first.text)
    assert_equal 'min_amount', @rule.reload.threshold_type
    assert_equal '10.0', @rule.reload.min_amount
  end

  test 'returns error when rule not found' do
    response = UpdateRuleSettingsTool.new(rule_id: 999_999, max_fee_percentage: 1.0).execute

    assert_match(/not found/i, response.contents.first.text)
  end

  test 'returns error when rule is active' do
    @rule.update!(status: :scheduled)
    response = UpdateRuleSettingsTool.new(rule_id: @rule.id, max_fee_percentage: 1.0).execute

    assert_match(/stopped/i, response.contents.first.text)
  end

  test 'returns error when no settings provided' do
    response = UpdateRuleSettingsTool.new(rule_id: @rule.id).execute

    assert_match(/no settings/i, response.contents.first.text)
  end

  test 'returns error when tool is disabled' do
    @user.set_mcp_tool_enabled('update_rule_settings', false)
    response = UpdateRuleSettingsTool.new(rule_id: @rule.id, max_fee_percentage: 1.0).execute

    assert_match(/disabled/, response.contents.first.text)
  end
end
