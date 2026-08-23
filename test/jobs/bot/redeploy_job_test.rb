require 'test_helper'

class Bot::RedeployJobTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_index, user: create(:user), with_api_key: true)
  end

  test 'a refusal is reported rather than dropped' do
    # The controller has already told the user it started, so a guard declining in the worker has to
    # leave a trace — otherwise a one-shot command vanishes with no explanation.
    @bot.stubs(:redeploy!).returns(Result::Failure.new('rebalance_pending'))

    Bot::RedeployJob.new.perform(@bot)

    assert @bot.bot_activity_logs.exists?(event: 'redeploy_not_started')
  end

  test 'a successful run logs no refusal' do
    @bot.stubs(:redeploy!).returns(Result::Success.new(placed: 2))

    Bot::RedeployJob.new.perform(@bot)

    assert_not @bot.bot_activity_logs.exists?(event: 'redeploy_not_started')
  end

  test 'an archived bot explains itself instead of leaving a false success standing' do
    @bot.update_columns(status: Bot.statuses[:archived])
    @bot.expects(:redeploy!).never

    Bot::RedeployJob.new.perform(@bot)

    assert @bot.bot_activity_logs.exists?(event: 'redeploy_not_started')
  end

  test 'a key still waiting for activation places nothing' do
    key = @bot.api_key
    key.stubs(:pending_activation?).returns(true)
    @bot.stubs(:api_key).returns(key)
    @bot.expects(:redeploy!).never

    Bot::RedeployJob.new.perform(@bot)

    assert @bot.bot_activity_logs.exists?(event: 'redeploy_not_started')
  end

  # The schedule is only one of the ways money reaches a composition bot. This leg is a peer of
  # rebalancing, which also runs while the DCA leg is stopped.
  test 'a stopped bot is not refused' do
    @bot.update_columns(status: Bot.statuses[:stopped], started_at: nil)
    @bot.expects(:redeploy!).returns(Result::Success.new(placed: 1))

    Bot::RedeployJob.new.perform(@bot)

    assert_not @bot.bot_activity_logs.exists?(event: 'redeploy_not_started')
  end

  test 'a raise is reported too, not just a refusal' do
    # Every DECLINING guard logs; an exception did not. The user was told it started and would
    # otherwise get a flash and nothing else, with the reason buried where only the operator sees it.
    @bot.stubs(:redeploy!).raises(RuntimeError, 'Failed to read balance: Invalid API-key')

    assert_raises(RuntimeError) { Bot::RedeployJob.new.perform(@bot) }

    log = @bot.bot_activity_logs.find_by(event: 'redeploy_failed')
    assert log
    assert_match(/Invalid API-key/, log.details['reason'])
  end

  test 'a bot type without the leg is left alone' do
    other = create(:dca_single_asset, user: @bot.user)

    Bot::RedeployJob.new.perform(other)

    assert_empty other.bot_activity_logs
  end

  # Same key and group as the DCA tick, the rebalance and the liquidation: without that they would
  # all run against each other's stale balances, and a surviving `placing` intent could not be read
  # as a dead worker.
  test 'it shares the exchange semaphore with the other legs' do
    assert_equal 'Bot::ActionJob', Bot::RedeployJob.concurrency_group
    assert_equal Bot::LiquidateExitedJob.concurrency_group, Bot::RedeployJob.concurrency_group
    assert_equal "exchange_#{@bot.exchange.name_id}", Bot::RedeployJob.concurrency_key.call(@bot)
  end
end
