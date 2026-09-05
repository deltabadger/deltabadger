# frozen_string_literal: true

require 'test_helper'

class BotLifecycleToolsTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @bot = create(:dca_single_asset, :stopped, user: @user)
    %w[delete_bot archive_bot unarchive_bot].each { |tool| @user.set_mcp_tool_enabled(tool, true) }
    stub_mcp_client(@user)
  end

  teardown { ActionMCP::Current.reset }

  test 'delete_bot deletes and then reports the bot gone' do
    assert_match(/deleted/, DeleteBotTool.new(bot_id: @bot.id).execute.contents.first.text)
    assert @bot.reload.deleted?
    assert_match(/Bot not found/, DeleteBotTool.new(bot_id: @bot.id).execute.contents.first.text)
  end

  test 'archive_bot archives and refuses a second time' do
    assert_match(/archived/, ArchiveBotTool.new(bot_id: @bot.id).execute.contents.first.text)
    assert @bot.reload.archived?
    assert_match(/already archived/, ArchiveBotTool.new(bot_id: @bot.id).execute.contents.first.text)
  end

  test 'unarchive_bot brings the bot back stopped and refuses one that is not archived' do
    assert_match(/is not archived/, UnarchiveBotTool.new(bot_id: @bot.id).execute.contents.first.text)
    BotApi::Bots::Archive.call(user: @user, bot_id: @bot.id)

    assert_match(/reactivated/, UnarchiveBotTool.new(bot_id: @bot.id).execute.contents.first.text)
    assert @bot.reload.stopped?
  end
end
