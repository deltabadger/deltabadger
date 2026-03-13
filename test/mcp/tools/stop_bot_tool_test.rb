# frozen_string_literal: true

require 'test_helper'

class StopBotToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @bot = create(:dca_single_asset, user: @user, status: :scheduled)
    ActionMCP::Current.user = @user
    AppConfig.set_mcp_tool_enabled('stop_bot', true)
  end

  teardown do
    ActionMCP::Current.reset
    AppConfig.delete(AppConfig::MCP_TOOL_PERMISSIONS)
  end

  test 'stops a running bot' do
    response = StopBotTool.new(bot_id: @bot.id).execute

    @bot.reload
    assert @bot.stopped?
    assert_match(/stopped/, response.contents.first.text)
  end

  test 'returns error for already stopped bot' do
    @bot.update!(status: :stopped)

    response = StopBotTool.new(bot_id: @bot.id).execute

    assert_match(/not running/, response.contents.first.text)
  end

  test 'returns error for non-existent bot' do
    response = StopBotTool.new(bot_id: 999_999).execute

    assert_match(/not found/, response.contents.first.text)
  end

  test 'returns error when tool is disabled' do
    AppConfig.set_mcp_tool_enabled('stop_bot', false)

    response = StopBotTool.new(bot_id: @bot.id).execute

    assert_match(/disabled/, response.contents.first.text)
    @bot.reload
    assert @bot.scheduled?
  end
end
