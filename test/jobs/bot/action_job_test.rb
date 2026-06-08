require 'test_helper'

# Shared behavior tests for Bot::ActionJob across both bot types
module ActionJobBehaviorTests
  extend ActiveSupport::Concern
  include ActiveJob::TestHelper
  include ActiveSupport::Testing::TimeHelpers

  # Subclasses must define #create_bot returning a started bot

  included do
    test 'executes the bot action when scheduled' do
      bot = create_bot
      setup_action_job_mocks(bot)
      bot.expects(:execute_action).returns(Result::Success.new)

      Bot::ActionJob.new.perform(bot)
    end

    test 'updates last_action_job_at' do
      bot = create_bot
      setup_action_job_mocks(bot)

      freeze_time do
        Bot::ActionJob.new.perform(bot)
        assert_equal Time.current, bot.reload.last_action_job_at
      end
    end

    test 'sets bot status to scheduled after execution' do
      bot = create_bot
      setup_action_job_mocks(bot)

      Bot::ActionJob.new.perform(bot)
      assert_equal 'scheduled', bot.reload.status
    end

    test 'schedules next action job at next_interval_checkpoint_at' do
      bot = create_bot
      setup_action_job_mocks(bot)

      job_setter = stub(perform_later: true)
      Bot::ActionJob.unstub(:set)
      Bot::ActionJob.expects(:set)
                    .with(wait_until: bot.next_interval_checkpoint_at)
                    .returns(job_setter)
      job_setter.expects(:perform_later).with(bot)

      Bot::ActionJob.new.perform(bot)
    end

    test 'broadcasts after scheduling' do
      bot = create_bot
      setup_action_job_mocks(bot)
      Bot::BroadcastAfterScheduledActionJob.expects(:perform_later).with(bot)

      Bot::ActionJob.new.perform(bot)
    end

    test 'sets waiting_for_market_open when market is closed' do
      bot = create_bot
      setup_action_job_mocks(bot)
      bot.exchange.stubs(:market_open?).returns(false)
      bot.exchange.stubs(:next_market_open_at).returns(1.hour.from_now)

      Bot::ActionJob.new.perform(bot)
      assert bot.reload.waiting_for_market_open
    end

    test 'clears waiting_for_market_open when market is open' do
      bot = create_bot
      bot.update!(waiting_for_market_open: true)
      setup_action_job_mocks(bot)

      Bot::ActionJob.new.perform(bot)
      assert_nil bot.reload.waiting_for_market_open
    end

    test 'executes action when bot is retrying' do
      bot = create_bot
      bot.update!(status: :retrying)
      setup_action_job_mocks(bot)
      bot.expects(:execute_action).returns(Result::Success.new)

      Bot::ActionJob.new.perform(bot)
    end

    test 'does not execute action when bot is stopped' do
      bot = create_bot
      bot.update!(status: :stopped)
      setup_action_job_mocks(bot)
      bot.expects(:execute_action).never

      Bot::ActionJob.new.perform(bot)
    end

    test 'raises error when bot already has a scheduled action job' do
      bot = create_bot
      setup_action_job_mocks(bot)
      bot.stubs(:next_action_job_at).returns(1.hour.from_now)

      error = assert_raises(RuntimeError) do
        Bot::ActionJob.new.perform(bot)
      end
      assert_match(/already has an action job scheduled/, error.message)
    end

    test 'raises error when execute_action fails' do
      bot = create_bot
      setup_action_job_mocks(bot)
      bot.stubs(:execute_action).returns(Result::Failure.new('Test error'))
      bot.stubs(:notify_about_error)

      assert_raises(RuntimeError, 'Test error') do
        Bot::ActionJob.new.perform(bot)
      end
    end

    test 'sets bot status to retrying when execute_action fails' do
      bot = create_bot
      setup_action_job_mocks(bot)
      bot.stubs(:execute_action).returns(Result::Failure.new('Test error'))
      bot.stubs(:notify_about_error)

      begin
        Bot::ActionJob.new.perform(bot)
      rescue StandardError
        nil
      end
      assert_equal 'retrying', bot.reload.status
    end

    test 'sends end_of_funds notification (not generic error) when error is insufficient_funds' do
      bot = create_bot
      setup_action_job_mocks(bot)
      bot.stubs(:execute_action).returns(Result::Failure.new('insufficient buying power'))
      bot.exchange.stubs(:known_errors).returns(insufficient_funds: ['insufficient buying power'])
      bot.expects(:notify_end_of_funds).once
      bot.expects(:notify_about_error).never

      Bot::ActionJob.new.perform(bot)
      assert_equal 'retrying', bot.reload.status
    end

    test 'does not re-raise when error is insufficient_funds' do
      bot = create_bot
      setup_action_job_mocks(bot)
      bot.stubs(:execute_action).returns(Result::Failure.new('insufficient buying power'))
      bot.exchange.stubs(:known_errors).returns(insufficient_funds: ['insufficient buying power'])
      bot.stubs(:notify_end_of_funds)

      assert_nothing_raised do
        Bot::ActionJob.new.perform(bot)
      end
    end

    test 'does not schedule next job when break_reschedule is true' do
      bot = create_bot
      setup_action_job_mocks(bot)
      bot.stubs(:execute_action).returns(Result::Success.new(break_reschedule: true))
      Bot::ActionJob.expects(:set).never

      Bot::ActionJob.new.perform(bot)
    end

    test 'does not update status when break_reschedule is true' do
      bot = create_bot
      setup_action_job_mocks(bot)
      original_status = bot.status
      bot.stubs(:execute_action).returns(Result::Success.new(break_reschedule: true))

      Bot::ActionJob.new.perform(bot)
      assert_equal original_status, bot.reload.status
    end

    test 'humanized_errors helper humanizes the raw error message' do
      bot = create_bot
      raw = 'EAccount:Invalid permissions:XAUT trading restricted for DK.'
      error = StandardError.new(raw)
      bot.exchange.stubs(:humanize_error).with(raw).returns('humanized message')

      assert_equal ['humanized message'], Bot::ActionJob.new.send(:humanized_errors, bot, error)
    end

    test 'notify_retry long-delay branch passes humanized message' do
      bot = create_bot
      setup_action_job_mocks(bot)
      raw = 'boom'
      bot.stubs(:execute_action).returns(Result::Failure.new(raw))
      bot.exchange.stubs(:humanize_error).with(raw).returns('humanized')
      bot.expects(:notify_about_error).with(errors: ['humanized'])

      job = Bot::ActionJob.new
      # estimated_retry_delay > 1.minute and <= effective_interval_duration -> line 78 branch
      job.stubs(:estimated_retry_delay).returns(2.minutes)

      begin
        job.perform(bot)
      rescue StandardError
        nil
      end
      assert_equal 'retrying', bot.reload.status
    end

    test 'notify_ignorable for a non-insufficient_funds category passes humanized message' do
      bot = create_bot
      raw = 'some other ignorable error'
      error = StandardError.new(raw)
      bot.exchange.stubs(:humanize_error).with(raw).returns('humanized')
      bot.expects(:notify_about_error).with(errors: ['humanized'])

      Bot::ActionJob.new.send(:notify_ignorable, bot, :some_other_category, error)
    end

    test 'notify_retry past-interval branch passes humanized message' do
      bot = create_bot
      setup_action_job_mocks(bot)
      raw = 'boom'
      bot.stubs(:execute_action).returns(Result::Failure.new(raw))
      bot.stubs(:effective_interval_duration).returns(10.seconds)
      bot.exchange.stubs(:humanize_error).with(raw).returns('humanized')
      bot.expects(:notify_about_error).with(errors: ['humanized'])

      job = Bot::ActionJob.new
      # estimated_retry_delay > effective_interval_duration -> line 76 branch
      job.stubs(:estimated_retry_delay).returns(1.minute)

      begin
        job.perform(bot)
      rescue StandardError
        nil
      end
      assert_equal 'retrying', bot.reload.status
    end

    # == Activity logging (Finding 3) ==

    test 'logs a market_closed activity when the market is closed' do
      bot = create_bot
      setup_action_job_mocks(bot)
      bot.exchange.stubs(:market_open?).returns(false)
      bot.exchange.stubs(:next_market_open_at).returns(1.hour.from_now)

      assert_difference -> { bot.bot_activity_logs.where(event: 'market_closed').count }, 1 do
        Bot::ActionJob.new.perform(bot)
      end
    end

    test 'logs an execution_failed activity tagged with the ignorable category' do
      bot = create_bot
      setup_action_job_mocks(bot)
      bot.stubs(:execute_action).returns(Result::Failure.new('insufficient buying power'))
      bot.exchange.stubs(:known_errors).returns(insufficient_funds: ['insufficient buying power'])
      bot.stubs(:notify_end_of_funds)

      assert_difference -> { bot.bot_activity_logs.where(event: 'execution_failed').count }, 1 do
        Bot::ActionJob.new.perform(bot)
      end

      log = bot.bot_activity_logs.where(event: 'execution_failed').last
      assert_equal 'error', log.level
      assert_equal 'insufficient_funds', log.details['ignorable']
    end

    test 'does not log execution_failed when a failed transaction was recorded this cycle' do
      freeze_time do
        bot = create_bot
        create(:transaction, bot: bot, status: :failed, external_status: :unknown, created_at: Time.current)
        setup_action_job_mocks(bot)
        bot.stubs(:execute_action).returns(Result::Failure.new('boom'))
        bot.stubs(:notify_about_error)

        assert_no_difference -> { bot.bot_activity_logs.where(event: 'execution_failed').count } do
          Bot::ActionJob.new.perform(bot)
        rescue StandardError
          nil
        end
      end
    end

    test 'logs execution_failed when the only failed transaction is from a previous cycle' do
      bot = create_bot
      create(:transaction, bot: bot, status: :failed, external_status: :unknown, created_at: 1.hour.ago)
      setup_action_job_mocks(bot)
      bot.stubs(:execute_action).returns(Result::Failure.new('boom'))
      bot.stubs(:notify_about_error)

      assert_difference -> { bot.bot_activity_logs.where(event: 'execution_failed').count }, 1 do
        Bot::ActionJob.new.perform(bot)
      rescue StandardError
        nil
      end
    end

    test 'logs a reschedule_disabled activity when break_reschedule is set' do
      bot = create_bot
      setup_action_job_mocks(bot)
      bot.stubs(:execute_action).returns(Result::Success.new(break_reschedule: true))

      assert_difference -> { bot.bot_activity_logs.where(event: 'reschedule_disabled').count }, 1 do
        Bot::ActionJob.new.perform(bot)
      end
    end
  end

  private

  def setup_action_job_mocks(bot)
    setup_bot_execution_mocks(bot)
    bot.stubs(:broadcast_below_minimums_warning)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
  end
end

class Bot::ActionJobWithSingleAssetTest < ActiveSupport::TestCase
  include ActionJobBehaviorTests

  private

  def create_bot
    create(:dca_single_asset, :started)
  end
end

class Bot::ActionJobWithDualAssetTest < ActiveSupport::TestCase
  include ActionJobBehaviorTests

  private

  def create_bot
    create(:dca_dual_asset, :started)
  end
end

class Bot::ActionJobQueueTest < ActiveSupport::TestCase
  test 'uses the exchange-specific queue' do
    bot = create(:dca_single_asset, :started)
    job = Bot::ActionJob.new(bot)
    assert_equal bot.exchange.name_id.to_sym, job.queue_name
  end
end

class Bot::ActionJobTransientNetworkTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper

  test 'declares retry_on Client::TransientNetworkError' do
    handler_classes = Bot::ActionJob.rescue_handlers.map(&:first)
    assert_includes handler_classes, 'Client::TransientNetworkError',
                    'Bot::ActionJob should declare retry_on Client::TransientNetworkError'
  end

  test 'retry handler is scoped to Bot::ActionJob and not inherited by sibling jobs' do
    # Sibling jobs that also inherit from ApplicationJob must NOT pick up the
    # bot-specific retry handler — otherwise the exhaustion block (which
    # assumes job.arguments.first is a Bot) would silently swallow their
    # transient failures.
    base_handlers = ApplicationJob.rescue_handlers.map(&:first)
    refute_includes base_handlers, 'Client::TransientNetworkError',
                    'ApplicationJob must not declare retry_on Client::TransientNetworkError'

    sibling_handlers = Exchange::SyncAlpacaAssetsJob.rescue_handlers.map(&:first)
    refute_includes sibling_handlers, 'Client::TransientNetworkError',
                    'Exchange::SyncAlpacaAssetsJob must not inherit retry_on Client::TransientNetworkError'
  end

  test 'bypass clause flips bot to :retrying and re-raises, without writing execution_failed' do
    bot = create(:dca_single_asset, :started)
    setup_bot_execution_mocks(bot)
    # Bot starts in :scheduled (factory default) so the line-8 guard admits the
    # perform. Mimic real execute_action: flip to :executing, then raise the
    # transient error mid-flight.
    bot.define_singleton_method(:execute_action) do
      update!(status: :executing)
      raise Client::TransientNetworkError, 'Net::OpenTimeout: TCP open timed out'
    end
    bot.expects(:notify_about_error).never
    bot.expects(:notify_end_of_funds).never

    # Call #perform directly (not perform_now) so retry_on at the ActiveJob layer
    # doesn't engage — we want to isolate the inner rescue clause.
    assert_raises(Client::TransientNetworkError) do
      Bot::ActionJob.new.perform(bot)
    end

    assert_equal 'retrying', bot.reload.status
    assert_equal 0, bot.bot_activity_logs.where(event: 'execution_failed').count,
                 'transient errors must not write an execution_failed activity'
  end

  test 'retried perform passes the line-8 guard because bypass left bot in :retrying' do
    bot = create(:dca_single_asset, :started)
    setup_action_job_mocks(bot)
    # Bot::Accountable's before_save guard requires set_missed_quote_amount to
    # run before settings-touching saves. The real execute_action handles that;
    # our stubbed Result::Success.new does not. The accounting plumbing is
    # orthogonal to what this test verifies, so neutralize the guard on these
    # bot instances only.
    bot.define_singleton_method(:check_missed_quote_amount_was_set) { nil }

    # First attempt: bot starts :scheduled (factory default). execute_action
    # flips to :executing and then raises — the bypass clause must transition
    # to :retrying so the retried perform passes the guard.
    bot.define_singleton_method(:execute_action) do
      update!(status: :executing)
      raise Client::TransientNetworkError, 'Net::OpenTimeout: x'
    end
    assert_raises(Client::TransientNetworkError) { Bot::ActionJob.new.perform(bot) }
    assert_equal 'retrying', bot.reload.status

    # Simulated retry: fresh perform, fresh bot instance (matches the way
    # ActiveJob deserializes args between retries). The guard at line 8 of
    # Bot::ActionJob#perform is `return unless bot.scheduled? || bot.retrying?`
    # so this must run (not short-circuit) and reach a successful execute_action.
    retried_bot = Bot.find(bot.id)
    retried_bot.define_singleton_method(:check_missed_quote_amount_was_set) { nil }
    setup_action_job_mocks(retried_bot)
    retried_bot.expects(:execute_action).returns(Result::Success.new).once

    Bot::ActionJob.new.perform(retried_bot)
    assert_equal 'scheduled', retried_bot.reload.status
  end

  test 'exhaustion handoff: flips to :retrying, logs execution_failed transient_exhausted, notifies, re-enqueues, broadcasts' do
    bot = create(:dca_single_asset, :started)
    setup_action_job_mocks(bot)
    bot.exchange.stubs(:humanize_error).with('Net::OpenTimeout: persistent').returns('Connection issue')
    bot.define_singleton_method(:execute_action) do
      update!(status: :executing)
      raise Client::TransientNetworkError, 'Net::OpenTimeout: persistent'
    end
    bot.expects(:notify_about_error).with(errors: ['Connection issue']).once

    job_setter = stub
    job_setter.expects(:perform_later).with(bot).once
    Bot::ActionJob.unstub(:set)
    Bot::ActionJob.expects(:set).with(wait_until: bot.next_interval_checkpoint_at).returns(job_setter)
    Bot::BroadcastAfterScheduledActionJob.unstub(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.expects(:perform_later).with(bot).once

    # retry_on yields to our exhaustion block when executions >= attempts.
    # ActiveJob tracks per-exception executions in `exception_executions`,
    # incremented inside `executions_for`. Pre-seed that counter so the next
    # failure pushes it past the attempts cap and the rescue chain yields to
    # our block end-to-end (no private-API pokes).
    job = Bot::ActionJob.new(bot)
    job.exception_executions['[Client::TransientNetworkError]'] = 4

    assert_nothing_raised { job.perform_now }

    assert_equal 'retrying', bot.reload.status
    log = bot.bot_activity_logs.where(event: 'execution_failed').last
    assert log, 'execution_failed activity must be logged on exhaustion'
    assert_equal 'error', log.level
    assert_equal true, log.details['transient_exhausted']
  end

  test 'declares retry_on Client::RateLimitedError' do
    handler_classes = Bot::ActionJob.rescue_handlers.map(&:first)
    assert_includes handler_classes, 'Client::RateLimitedError',
                    'Bot::ActionJob should declare retry_on Client::RateLimitedError'
  end

  test 'rate-limit retry handler is scoped to Bot::ActionJob and not inherited by sibling jobs' do
    base_handlers = ApplicationJob.rescue_handlers.map(&:first)
    refute_includes base_handlers, 'Client::RateLimitedError',
                    'ApplicationJob must not declare retry_on Client::RateLimitedError'

    sibling_handlers = Exchange::SyncAlpacaAssetsJob.rescue_handlers.map(&:first)
    refute_includes sibling_handlers, 'Client::RateLimitedError',
                    'Exchange::SyncAlpacaAssetsJob must not inherit retry_on Client::RateLimitedError'
  end

  # The wait must ESCALATE between attempts (a fixed/short wait re-trips Kraken's
  # decaying counter). Pin the shared lambda directly so a regression to a constant
  # wait fails loudly. ActiveJob passes the 1-based attempt count.
  test 'rate-limit retry wait escalates with each attempt' do
    assert_equal 15.seconds, BotJob::RATE_LIMIT_WAIT.call(1)
    assert_equal 30.seconds, BotJob::RATE_LIMIT_WAIT.call(2)
    assert_equal 45.seconds, BotJob::RATE_LIMIT_WAIT.call(3)
  end

  # Mirror of the transient bypass clause: a rate-limit error raised mid-flight must flip
  # the bot to :retrying and re-raise WITHOUT the noisy execution_failed / notify path,
  # so the ActiveJob retry chain handles it quietly (the whole point of the dedicated rescue).
  test 'rate-limit bypass clause flips bot to :retrying and re-raises, without writing execution_failed' do
    bot = create(:dca_single_asset, :started)
    setup_bot_execution_mocks(bot)
    bot.define_singleton_method(:execute_action) do
      update!(status: :executing)
      raise Client::RateLimitedError, 'EAPI:Rate limit exceeded'
    end
    bot.expects(:notify_about_error).never
    bot.expects(:notify_end_of_funds).never

    assert_raises(Client::RateLimitedError) do
      Bot::ActionJob.new.perform(bot)
    end

    assert_equal 'retrying', bot.reload.status
    assert_equal 0, bot.bot_activity_logs.where(event: 'execution_failed').count,
                 'rate-limit errors must not write an execution_failed activity mid-retry'
  end

  # On exhaustion the handoff mirrors transient, but the activity detail must be labeled
  # as rate-limited (NOT transient_exhausted) so the two failure modes stay distinguishable.
  test 'rate-limit exhaustion handoff: :retrying, logs execution_failed labeled rate_limited (not transient), notifies, re-enqueues' do
    bot = create(:dca_single_asset, :started)
    setup_action_job_mocks(bot)
    bot.exchange.stubs(:humanize_error).with('EAPI:Rate limit exceeded').returns('Exchange busy, retrying')
    bot.define_singleton_method(:execute_action) do
      update!(status: :executing)
      raise Client::RateLimitedError, 'EAPI:Rate limit exceeded'
    end
    bot.expects(:notify_about_error).with(errors: ['Exchange busy, retrying']).once

    job_setter = stub
    job_setter.expects(:perform_later).with(bot).once
    Bot::ActionJob.unstub(:set)
    Bot::ActionJob.expects(:set).with(wait_until: bot.next_interval_checkpoint_at).returns(job_setter)
    Bot::BroadcastAfterScheduledActionJob.unstub(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.expects(:perform_later).with(bot).once

    job = Bot::ActionJob.new(bot)
    job.exception_executions['[Client::RateLimitedError]'] = 4

    assert_nothing_raised { job.perform_now }

    assert_equal 'retrying', bot.reload.status
    log = bot.bot_activity_logs.where(event: 'execution_failed').last
    assert log, 'execution_failed activity must be logged on rate-limit exhaustion'
    assert_equal 'error', log.level
    assert_equal true, log.details['rate_limited_exhausted']
    assert_nil log.details['transient_exhausted'], 'rate-limit exhaustion must not be mislabeled transient'
  end

  private

  def setup_action_job_mocks(bot)
    setup_bot_execution_mocks(bot)
    bot.stubs(:broadcast_below_minimums_warning)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
  end
end

class Bot::ActionJobSchedulingIntegrationTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

  test 'creates a scheduled job in SolidQueue' do
    bot = create(:dca_single_asset, :started)
    setup_bot_execution_mocks(bot)
    bot.stubs(:broadcast_below_minimums_warning)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    freeze_time do
      SolidQueue::Job.destroy_all

      Bot::ActionJob.new.perform(bot)

      scheduled_job = SolidQueue::Job.find_by(class_name: 'Bot::ActionJob')
      assert scheduled_job.present?
    end
  end
end
