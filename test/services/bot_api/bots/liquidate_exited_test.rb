# frozen_string_literal: true

require 'test_helper'

class BotApi::Bots::LiquidateExitedTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @bot = create(:dca_index, user: @user, status: :scheduled, started_at: Time.current, with_api_key: true)
    Bots::DcaIndex.any_instance.stubs(:held_symbols).returns(%w[DOGE])
    Bots::DcaIndex.any_instance.stubs(:ensure_exchange_authenticated)
    Exchanges::Kraken.any_instance.stubs(:market_open?).returns(true)
  end

  test 'enqueues the sale and logs the request' do
    Bot::LiquidateExitedJob.expects(:perform_later).with(@bot, symbols: %w[DOGE], selling_token: anything)

    result = BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol: 'DOGE')

    assert result.success?, result.error_message
    assert_equal :accepted, result.status
    assert_equal 'liquidation_requested', @bot.bot_activity_logs.last.event
    assert_equal @user.id, @bot.bot_activity_logs.last.details['user_id']
    assert_equal 'DOGE', @bot.bot_activity_logs.last.details['base']
  end

  test 'a symbol the bot does not hold cannot be sold' do
    Bot::LiquidateExitedJob.expects(:perform_later).never
    assert_equal 'holding_not_held', BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol: 'BTC').error_code
  end

  test 'several symbols are queued as one job' do
    # Not one job per symbol: liquidation_blocked_reason refuses while an earlier sale's order is
    # still working, so jobs 2..N would be declined after the caller was told the sale started.
    Bots::DcaIndex.any_instance.stubs(:held_symbols).returns(%w[DOGE SHIB])
    Bot::LiquidateExitedJob.expects(:perform_later).with(@bot, symbols: %w[DOGE SHIB], selling_token: anything).once

    result = BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol: %w[DOGE SHIB])

    assert result.success?, result.error_message
    assert_equal %w[DOGE SHIB], result.data[:symbols]
    assert_equal 'DOGE, SHIB', @bot.bot_activity_logs.last.details['base']
  end

  test 'a one-symbol sale answers exactly as it always has' do
    # `symbol` is the REST body's field and the MCP sentence's subject; a one-element join IS that
    # element, so nothing downstream sees a change.
    Bot::LiquidateExitedJob.expects(:perform_later)

    result = BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol: 'DOGE')

    assert_equal 'DOGE', result.data[:symbol]
  end

  test 'a list containing an unheld position is refused whole' do
    # No confirmation step here in which to show a reduced list, unlike the page.
    Bot::LiquidateExitedJob.expects(:perform_later).never

    result = BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol: %w[DOGE ZZZ])

    assert_equal 'holding_not_held', result.error_code
    assert_match(/ZZZ/, result.error_message)
  end

  test 'naming nothing sells nothing' do
    Bot::LiquidateExitedJob.expects(:perform_later).never

    assert_equal 'holding_not_held',
                 BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol: []).error_code
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

  test 'the request itself marks the bot as selling, and repaints' do
    # The job queues behind the exchange semaphore, so liquidation_in_flight? is still false when
    # this returns. Without the marker the page goes on offering Sell for the whole wait — which is
    # exactly when someone clicks it again.
    Bot::LiquidateExitedJob.stubs(:perform_later)
    Bots::DcaIndex.any_instance.expects(:broadcast_selling_state).once

    BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol: 'DOGE')

    assert @bot.reload.liquidation_selling?
  end

  test 'the token handed to the job is the one that can clear the marker' do
    token = nil
    Bot::LiquidateExitedJob.stubs(:perform_later).with { |_bot, kw| token = kw[:selling_token] }

    BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol: 'DOGE')

    assert token.present?
    assert @bot.reload.clear_selling!(token)
    assert_not @bot.liquidation_selling?
  end

  test 'a dry run marks nothing' do
    BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol: 'DOGE', dry_run: true)

    assert_not @bot.reload.liquidation_selling?
  end

  test 'a refused sale marks nothing' do
    BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol: 'BTC')

    assert_not @bot.reload.liquidation_selling?
  end

  test 'the repaint lands before the job is queued, so a fast worker cannot be painted over' do
    # A job that ran to completion in between would clear the marker and broadcast the idle tables;
    # a repaint after the enqueue, from an instance still holding the marker, would put the spinners
    # back over a sale that was already done and leave nothing to take them down.
    order = []
    Bots::DcaIndex.any_instance.stubs(:broadcast_selling_state).with { |*| order << :repaint }
    Bot::LiquidateExitedJob.stubs(:perform_later).with { |*| order << :enqueue }

    BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol: 'DOGE')

    assert_equal %i[repaint enqueue], order
  end

  test 'an enqueue that fails takes its own marker back down' do
    # The marker is cleared by the JOB. A failure that produced no job leaves nothing to clear it,
    # so the page would spin for the whole TTL over a sale that never started.
    Bot::LiquidateExitedJob.stubs(:perform_later).raises(RuntimeError, 'queue unavailable')

    assert_raises(RuntimeError) { BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol: 'DOGE') }

    assert_not @bot.reload.liquidation_selling?
  end
end
