# frozen_string_literal: true

require 'test_helper'

class UpdateBotSettingsToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @bot = create(:dca_single_asset, user: @user, status: :stopped)
    ActionMCP::Current.user = @user
    @user.set_mcp_tool_enabled('update_bot_settings', true)
  end

  teardown do
    ActionMCP::Current.reset
  end

  test 'updates quote_amount on a stopped bot' do
    response = UpdateBotSettingsTool.new(bot_id: @bot.id, quote_amount: 50.0).execute

    @bot.reload
    assert_equal 50.0, @bot.quote_amount.to_f
    assert_match(/updated/, response.contents.first.text)
  end

  test 'updates label' do
    UpdateBotSettingsTool.new(bot_id: @bot.id, label: 'My BTC Bot').execute

    @bot.reload
    assert_equal 'My BTC Bot', @bot.label
  end

  test 'rejects update on a running bot' do
    @bot.update!(status: :scheduled)

    response = UpdateBotSettingsTool.new(bot_id: @bot.id, quote_amount: 50.0).execute

    assert_match(/must be stopped/, response.contents.first.text)
  end

  test 'returns error for non-existent bot' do
    response = UpdateBotSettingsTool.new(bot_id: 999_999, quote_amount: 50.0).execute

    assert_match(/not found/, response.contents.first.text)
  end

  test 'returns error when tool is disabled' do
    @user.set_mcp_tool_enabled('update_bot_settings', false)

    response = UpdateBotSettingsTool.new(bot_id: @bot.id, quote_amount: 50.0).execute

    assert_match(/disabled/, response.contents.first.text)
  end

  test 'rejects when no settings provided' do
    response = UpdateBotSettingsTool.new(bot_id: @bot.id).execute

    assert_match(/No settings provided/, response.contents.first.text)
  end
end
