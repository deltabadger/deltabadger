# frozen_string_literal: true

require 'test_helper'

class BotApi::Bots::LiquidateExitedTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @bot = create(:dca_index, user: @user, status: :scheduled, started_at: Time.current, with_api_key: true)
    Bots::DcaIndex.any_instance.stubs(:exited_symbols).returns(%w[DOGE])
    Bots::DcaIndex.any_instance.stubs(:ensure_exchange_authenticated)
    Exchanges::Kraken.any_instance.stubs(:market_open?).returns(true)
  end

  test 'enqueues the sale and logs the request' do
    Bot::LiquidateExitedJob.expects(:perform_later).with(@bot, symbol: 'DOGE')

    result = BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol: 'DOGE')

    assert result.success?, result.error_message
    assert_equal :accepted, result.status
    assert_equal 'liquidation_requested', @bot.bot_activity_logs.last.event
    assert_equal @user.id, @bot.bot_activity_logs.last.details['user_id']
    assert_equal 'DOGE', @bot.bot_activity_logs.last.details['base']
  end

  test 'a holding the composition still owns cannot be sold this way' do
    Bot::LiquidateExitedJob.expects(:perform_later).never
    assert_equal 'holding_not_exited', BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol: 'BTC').error_code
  end

  test 'only composition bots, and not archived ones' do
    single = create(:dca_single_asset, :stopped, user: @user)
    assert_equal 'not_composition_bot',
                 BotApi::Bots::LiquidateExited.call(user: @user, bot_id: single.id, symbol: 'BTC').error_code
    @bot.update!(status: :archived)
    Bot::LiquidateExitedJob.expects(:perform_later).never
    assert_equal 'bot_archived', BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol: 'DOGE').error_code
  end

  test 'a closed market is refused; a failed check is not' do
    Exchanges::Kraken.any_instance.stubs(:market_open?).returns(false)
    assert_equal 'market_closed', BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol: 'DOGE').error_code
    Exchanges::Kraken.any_instance.stubs(:market_open?).raises(StandardError, 'boom')
    Bot::LiquidateExitedJob.stubs(:perform_later)
    assert BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol: 'DOGE').success?
  end

  test 'dry run validates and enqueues nothing' do
    Bot::LiquidateExitedJob.expects(:perform_later).never
    result = BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol: 'DOGE', dry_run: true)
    assert result.success?
    assert result.data[:dry_run]
    assert_equal 0, @bot.bot_activity_logs.count
  end

  test 'an unknown bot is a 404' do
    assert_equal 'bot_not_found', BotApi::Bots::LiquidateExited.call(user: @user, bot_id: 0, symbol: 'DOGE').error_code
  end
end
