# frozen_string_literal: true

require 'test_helper'

class StartBotToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @bot = create(:dca_single_asset, user: @user, status: :stopped)
    @user.set_mcp_tool_enabled('start_bot', true)
    stub_mcp_client(@user)
  end

  teardown do
    ActionMCP::Current.reset
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
    @user.set_mcp_tool_enabled('start_bot', false)

    response = StartBotTool.new(bot_id: @bot.id).execute

    assert_match(/disabled/, response.contents.first.text)
    @bot.reload
    assert @bot.stopped?
  end

  test 'paper trading on: refuses to start the bot and schedules nothing' do
    @user.mcp_dry_run = true
    Bot::ActionJob.expects(:perform_later).never
    Bot::BroadcastAfterScheduledActionJob.expects(:perform_later).never

    response = StartBotTool.new(bot_id: @bot.id).execute

    assert @bot.reload.stopped?
    assert_match(/\A\[DRY RUN\] .*nothing was created or started/i, response.contents.first.text)
  end

  test 'offers the start modal choice as start_fresh' do
    schema = StartBotTool.to_h[:inputSchema]
    assert_equal 'boolean', schema[:properties]['start_fresh'][:type]
    assert_equal ['bot_id'], schema[:required]
  end

  test 'start_fresh: false continues where the bot stopped' do
    @bot.update_columns(started_at: 3.days.ago)
    Bot::ActionJob.stubs(:perform_later)
    Bot::ActionJob.stubs(:set).returns(Bot::ActionJob)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    started = @bot.reload.started_at
    StartBotTool.new(bot_id: @bot.id, start_fresh: false).execute

    assert @bot.reload.working?
    assert_equal started, @bot.started_at
  end

  test 'no start_fresh starts fresh, as the Start button does' do
    @bot.update_columns(started_at: 3.days.ago)
    Bot::ActionJob.stubs(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    freeze_time do
      StartBotTool.new(bot_id: @bot.id).execute
      assert_equal Time.current, @bot.reload.started_at
    end
  end
end
