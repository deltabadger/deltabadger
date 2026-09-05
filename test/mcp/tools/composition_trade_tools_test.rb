# frozen_string_literal: true

require 'test_helper'

# liquidate_exited_asset and answer_redeploy_offer are trade tools, so they honour paper trading:
# with dry run on they validate everything and queue nothing.
class CompositionTradeToolsTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @bot = create(:dca_index, user: @user, status: :scheduled, started_at: Time.current, with_api_key: true)
    Bots::DcaIndex.any_instance.stubs(:exited_symbols).returns(%w[DOGE])
    Bots::DcaIndex.any_instance.stubs(:redeploy_offer).returns(25.to_d)
    Bots::DcaIndex.any_instance.stubs(:ensure_exchange_authenticated)
    Bots::DcaIndex.any_instance.stubs(:composition_tickers).returns([])
    Exchanges::Kraken.any_instance.stubs(:market_open?).returns(true)
    %w[liquidate_exited_asset answer_redeploy_offer].each { |tool| @user.set_mcp_tool_enabled(tool, true) }
    stub_mcp_client(@user)
  end

  teardown do
    ActionMCP::Current.reset
    Thread.current[:force_dry_run] = nil
  end

  test 'liquidate_exited_asset queues the sale' do
    Bot::LiquidateExitedJob.expects(:perform_later).with(@bot, symbol: 'DOGE')

    text = LiquidateExitedAssetTool.new(bot_id: @bot.id, symbol: 'DOGE').execute.contents.first.text

    assert_match(/Selling DOGE/, text)
    assert_no_match(/DRY RUN/, text)
  end

  test 'liquidate_exited_asset refuses a holding that is still in the composition' do
    Bot::LiquidateExitedJob.expects(:perform_later).never

    text = LiquidateExitedAssetTool.new(bot_id: @bot.id, symbol: 'BTC').execute.contents.first.text

    assert_match(/not a holding this bot's composition has dropped/, text)
  end

  test 'paper trading validates the sale and queues nothing' do
    @user.mcp_dry_run = true
    Bot::LiquidateExitedJob.expects(:perform_later).never

    text = LiquidateExitedAssetTool.new(bot_id: @bot.id, symbol: 'DOGE').execute.contents.first.text

    assert_match(/\A\[DRY RUN\] Would sell DOGE/, text)
    assert_match(/nothing was queued/, text)
  end

  test 'answer_redeploy_offer accepts and declines' do
    Bot::RedeployJob.expects(:perform_later).with(@bot)
    assert_match(/Redeploying 25\.0/, AnswerRedeployOfferTool.new(bot_id: @bot.id, accept: true).execute.contents.first.text)

    Bot::DeclineRedeployJob.expects(:perform_later).with(@bot, user_id: @user.id)
    text = AnswerRedeployOfferTool.new(bot_id: @bot.id, accept: false).execute.contents.first.text
    assert_match(/Declined redeploying/, text)
  end

  test 'paper trading answers the offer without queueing' do
    @user.mcp_dry_run = true
    Bot::RedeployJob.expects(:perform_later).never

    text = AnswerRedeployOfferTool.new(bot_id: @bot.id, accept: true).execute.contents.first.text

    assert_match(/\A\[DRY RUN\] Would redeploy 25\.0/, text)
  end
end
