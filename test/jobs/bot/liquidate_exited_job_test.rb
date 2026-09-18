require 'test_helper'

class Bot::LiquidateExitedJobTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_index, user: create(:user), with_api_key: true)
    @bot.stubs(:liquidation_tickers).returns([])
  end

  test 'a refusal is reported rather than dropped' do
    # The controller has already told the user that selling started, so a guard declining in the
    # worker has to leave a trace — otherwise a one-shot command vanishes with no explanation.
    @bot.stubs(:liquidate!).returns(Result::Failure.new('rebalance_pending'))

    Bot::LiquidateExitedJob.new.perform(@bot, holdings: [['CCC', 3]])

    assert @bot.bot_activity_logs.exists?(event: 'liquidation_not_started')
  end

  test 'a successful run logs no refusal' do
    @bot.stubs(:liquidate!).returns(Result::Success.new(placed: 1))

    Bot::LiquidateExitedJob.new.perform(@bot, holdings: [['CCC', 3]])

    assert_not @bot.bot_activity_logs.exists?(event: 'liquidation_not_started')
  end

  test 'a closed market says why, and sells nothing' do
    @bot.exchange.stubs(:market_open?).returns(false)
    @bot.expects(:liquidate!).never

    Bot::LiquidateExitedJob.new.perform(@bot, holdings: [['CCC', 3]])

    assert @bot.bot_activity_logs.exists?(event: 'liquidation_market_closed')
  end

  test 'an archived bot explains itself instead of leaving a false success standing' do
    # The controller has already told the user the sale started.
    @bot.update_columns(status: Bot.statuses[:archived])
    @bot.expects(:liquidate!).never

    Bot::LiquidateExitedJob.new.perform(@bot, holdings: [['CCC', 3]])

    assert @bot.bot_activity_logs.exists?(event: 'liquidation_not_started')
  end

  test 'a raise is reported too, not just a refusal' do
    # The gap this closes: every DECLINING guard already logs, but an exception did not — a rejected
    # API key, a rate limit, a balance read that failed. The user was told the sale started and got
    # a flash and nothing else, with the reason buried in solid_queue_failed_executions.
    @bot.stubs(:liquidate!).raises(RuntimeError, 'Failed to read balance: Invalid API-key')

    assert_raises(RuntimeError) { Bot::LiquidateExitedJob.new.perform(@bot, holdings: [['CCC', 3]]) }

    log = @bot.bot_activity_logs.find_by(event: 'liquidation_failed')
    assert log, 'a sale that died has to say so where the user looks'
    assert_match(/Invalid API-key/, log.details['reason'])
    # Its own event, not the refusal one — whose wording ("the bot was busy, or the index could not
    # be refreshed") would be a wrong explanation for a rejected key.
    assert_not @bot.bot_activity_logs.exists?(event: 'liquidation_not_started')
    assert_match(/Invalid API-key/, ApplicationController.helpers.bot_activity_summary(log))
  end

  test 'a bot type that cannot have quitters is refused' do
    other = create(:dca_single_asset, user: create(:user))

    assert_nothing_raised { Bot::LiquidateExitedJob.new.perform(other, holdings: [['CCC', 3]]) }
  end

  test 'the closed-market check is asked about every symbol in the batch' do
    @bot.unstub(:liquidation_tickers)
    @bot.expects(:liquidation_tickers).with(holdings: [['CCC', 3], ['DDD', 4]]).returns([])
    @bot.stubs(:liquidate!).returns(Result::Success.new(placed: 2))

    Bot::LiquidateExitedJob.new.perform(@bot, holdings: [['CCC', 3], ['DDD', 4]])
  end

  test 'the batch is given the semaphore lease as its deadline, not a window of its own' do
    # The lease is taken at DISPATCH and never renewed, so queue delay spends it too. A fixed window
    # measured from the start of the run would not notice, and the batch would go on starting
    # holdings after the exclusion it relies on had lapsed.
    expires = 90.seconds.from_now
    job = Bot::LiquidateExitedJob.new(@bot, holdings: [['CCC', 3]])
    SolidQueue::Semaphore.create!(key: job.concurrency_key, value: 0, expires_at: expires)
    @bot.expects(:liquidate!)
        .with { |args| args[:deadline].between?(expires - 46.seconds, expires - 44.seconds) }
        .returns(Result::Success.new(placed: 1))

    job.perform(@bot, holdings: [['CCC', 3]])
  end

  test 'no lease to read leaves the model its own window' do
    @bot.expects(:liquidate!).with(holdings: [['CCC', 3]], deadline: nil).returns(Result::Success.new(placed: 1))

    Bot::LiquidateExitedJob.new.perform(@bot, holdings: [['CCC', 3]])
  end

  test 'a sale enqueued before the plural rename still runs' do
    # These arguments are serialised in solid_queue_jobs. A keyword mismatch raises OUTSIDE the
    # rescue in perform, so the user would be told the sale started and get no activity row at all.
    @bot.expects(:liquidate!).with(holdings: [['CCC', nil]], deadline: nil).returns(Result::Success.new(placed: 1))

    Bot::LiquidateExitedJob.new.perform(@bot, symbol: 'CCC')
  end

  # --- the page's "a sale is under way" marker -------------------------------------------------
  #
  # Written in the request (the job queues behind the exchange semaphore for an unbounded wait), so
  # every way out of this job has to take it down again or the spinner outlives the sale.

  test 'a successful run takes the marker down' do
    token = @bot.mark_selling!
    @bot.stubs(:liquidate!).returns(Result::Success.new(placed: 1))

    Bot::LiquidateExitedJob.new.perform(@bot, holdings: [['CCC', 3]], selling_token: token)

    assert_not @bot.reload.liquidation_selling?
  end

  test 'a refusal takes the marker down too' do
    token = @bot.mark_selling!
    @bot.stubs(:liquidate!).returns(Result::Failure.new('rebalance_pending'))

    Bot::LiquidateExitedJob.new.perform(@bot, holdings: [['CCC', 3]], selling_token: token)

    assert_not @bot.reload.liquidation_selling?
  end

  test 'an archived bot never reaches liquidate! and still takes the marker down' do
    token = @bot.mark_selling!
    @bot.update_columns(status: Bot.statuses[:archived])

    Bot::LiquidateExitedJob.new.perform(@bot, holdings: [['CCC', 3]], selling_token: token)

    assert_not @bot.reload.liquidation_selling?
  end

  test 'a closed market takes the marker down' do
    token = @bot.mark_selling!
    @bot.exchange.stubs(:market_open?).returns(false)

    Bot::LiquidateExitedJob.new.perform(@bot, holdings: [['CCC', 3]], selling_token: token)

    assert_not @bot.reload.liquidation_selling?
  end

  test 'a raise takes the marker down on its way out' do
    # The ensure runs before the re-raise. Without it a crashed sale would spin until the TTL.
    token = @bot.mark_selling!
    @bot.stubs(:liquidate!).raises(RuntimeError, 'boom')

    assert_raises(RuntimeError) { Bot::LiquidateExitedJob.new.perform(@bot, holdings: [['CCC', 3]], selling_token: token) }

    assert_not @bot.reload.liquidation_selling?
  end

  test 'a job whose token has been superseded leaves the newer marker standing' do
    # Two requests queued before either placed. The first to finish must not drop the spinner for
    # the sale still coming.
    stale = @bot.mark_selling!
    Bot.find(@bot.id).mark_selling!
    @bot.stubs(:liquidate!).returns(Result::Success.new(placed: 1))
    @bot.expects(:broadcast_selling_state).never

    Bot::LiquidateExitedJob.new.perform(@bot, holdings: [['CCC', 3]], selling_token: stale)

    assert Bot.find(@bot.id).liquidation_selling?
  end

  test 'a sale enqueued before the marker shipped carries no token and still runs' do
    @bot.expects(:liquidate!).with(holdings: [['CCC', 3]], deadline: nil).returns(Result::Success.new(placed: 1))

    assert_nothing_raised { Bot::LiquidateExitedJob.new.perform(@bot, holdings: [['CCC', 3]]) }
  end
end
