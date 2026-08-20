require 'test_helper'

class Bot::LiquidateExitedJobTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_index, user: create(:user), with_api_key: true)
    @bot.stubs(:liquidation_tickers).returns([])
  end

  test 'a refusal is reported rather than dropped' do
    # The controller has already told the user that selling started, so a guard declining in the
    # worker has to leave a trace — otherwise a one-shot command vanishes with no explanation.
    @bot.stubs(:liquidate_exited!).returns(Result::Failure.new('rebalance_pending'))

    Bot::LiquidateExitedJob.new.perform(@bot)

    assert @bot.bot_activity_logs.exists?(event: 'liquidation_not_started')
  end

  test 'a successful run logs no refusal' do
    @bot.stubs(:liquidate_exited!).returns(Result::Success.new(placed: 1))

    Bot::LiquidateExitedJob.new.perform(@bot)

    assert_not @bot.bot_activity_logs.exists?(event: 'liquidation_not_started')
  end

  test 'a closed market says why, and sells nothing' do
    @bot.exchange.stubs(:market_open?).returns(false)
    @bot.expects(:liquidate_exited!).never

    Bot::LiquidateExitedJob.new.perform(@bot)

    assert @bot.bot_activity_logs.exists?(event: 'liquidation_market_closed')
  end

  test 'an archived bot explains itself instead of leaving a false success standing' do
    # The controller has already told the user the sale started.
    @bot.update_columns(status: Bot.statuses[:archived])
    @bot.expects(:liquidate_exited!).never

    Bot::LiquidateExitedJob.new.perform(@bot)

    assert @bot.bot_activity_logs.exists?(event: 'liquidation_not_started')
  end

  test 'a bot type that cannot have quitters is refused' do
    other = create(:dca_single_asset, user: create(:user))
    other.expects(:liquidate_exited!).never

    assert_nothing_raised { Bot::LiquidateExitedJob.new.perform(other) }
  end
end
