# frozen_string_literal: true

require 'test_helper'

class StartBotToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @bot = create(:dca_single_asset, user: @user, status: :stopped)
    ActionMCP::Current.user = @user
    AppConfig.set_mcp_tool_enabled('start_bot', true)
  end

  teardown do
    ActionMCP::Current.reset
    AppConfig.delete(AppConfig::MCP_TOOL_PERMISSIONS)
  end

  test 'starts a stopped bot' do
    Bot::ActionJob.stubs(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    response = StartBotTool.new(bot_id: @bot.id).execute

    @bot.reload
    assert @bot.working?
    assert_match(/started/, response.contents.first.text)
  end

  test 'returns error for already running bot' do
    @bot.update!(status: :scheduled)

    response = StartBotTool.new(bot_id: @bot.id).execute

    assert_match(/already running/, response.contents.first.text)
  end

  test 'returns error for non-existent bot' do
    response = StartBotTool.new(bot_id: 999_999).execute

    assert_match(/not found/, response.contents.first.text)
  end

  test 'returns error when tool is disabled' do
    AppConfig.set_mcp_tool_enabled('start_bot', false)

    response = StartBotTool.new(bot_id: @bot.id).execute

    assert_match(/disabled/, response.contents.first.text)
    @bot.reload
    assert @bot.stopped?
  end
end
