require 'test_helper'

# Characterization tests for the four limit-check jobs, written before extracting
# Bot::LimitCheckJobBase. The four jobs share one template and differ only in the
# condition method they poll and the time of the next check.
module LimitCheckJobBehaviorTests
  extend ActiveSupport::Concern
  include ActiveSupport::Testing::TimeHelpers

  # Including classes define: job_class, condition_method, expected_next_check_at(bot)
  # and optionally prepare_bot(bot) for type-specific stubs.

  included do
    test 'does nothing when the bot is not waiting' do
      bot = create_waiting_bot
      bot.update!(status: :stopped)
      bot.expects(condition_method).never
      job_class.expects(:set).never
      Bot::ActionJob.expects(:perform_later).never

      job_class.new.perform(bot)
    end

    test 'reschedules itself in 1 minute when the condition check fails' do
      bot = create_waiting_bot
      bot.stubs(condition_method).returns(Result::Failure.new('boom'))

      freeze_time do
        job_setter = mock
        job_setter.expects(:perform_later).with(bot)
        job_class.expects(:set).with(wait_until: 1.minute.from_now).returns(job_setter)

        job_class.new.perform(bot)
      end
      assert_equal 'waiting', bot.reload.status
    end

    test 'reschedules itself in 1 minute when the condition check RAISES a transient network error' do
      bot = create_waiting_bot
      bot.stubs(condition_method).raises(Client::TransientNetworkError, 'data.alpaca.markets timeout')

      freeze_time do
        job_setter = mock
        job_setter.expects(:perform_later).with(bot)
        job_class.expects(:set).with(wait_until: 1.minute.from_now).returns(job_setter)
        Bot::ActionJob.expects(:perform_later).never

        assert_nothing_raised { job_class.new.perform(bot) }
      end
      assert_equal 'waiting', bot.reload.status
    end

    test 'reschedules itself in 1 minute when the condition check RAISES a rate-limit error' do
      bot = create_waiting_bot
      bot.stubs(condition_method).raises(Client::RateLimitedError, 'EAPI:Rate limit exceeded')

      freeze_time do
        job_setter = mock
        job_setter.expects(:perform_later).with(bot)
        job_class.expects(:set).with(wait_until: 1.minute.from_now).returns(job_setter)
        Bot::ActionJob.expects(:perform_later).never

        assert_nothing_raised { job_class.new.perform(bot) }
      end
      assert_equal 'waiting', bot.reload.status
    end

    test 'does NOT swallow a non-transient error (still raises, dead-letters)' do
      bot = create_waiting_bot
      bot.stubs(condition_method).raises(StandardError, 'genuine bug')
      job_class.expects(:set).never

      assert_raises(StandardError) { job_class.new.perform(bot) }
    end

    test 'transitions the bot to scheduled and enqueues ActionJob when the condition is met' do
      bot = create_waiting_bot
      bot.stubs(condition_method).returns(Result::Success.new(true))
      Bot::ActionJob.expects(:perform_later).with(bot)
      job_class.expects(:set).never

      job_class.new.perform(bot)
      assert_equal 'scheduled', bot.reload.status
    end

    test 'reschedules itself at the type-specific next check time when the condition is not met' do
      bot = create_waiting_bot
      bot.stubs(condition_method).returns(Result::Success.new(false))

      freeze_time do
        job_setter = mock
        job_setter.expects(:perform_later).with(bot)
        job_class.expects(:set).with(wait_until: expected_next_check_at(bot)).returns(job_setter)
        Bot::ActionJob.expects(:perform_later).never

        job_class.new.perform(bot)
      end
      assert_equal 'waiting', bot.reload.status
    end
  end

  private

  def create_waiting_bot
    bot = create(:dca_single_asset, :started)
    bot.update!(status: :waiting)
    prepare_bot(bot)
    bot
  end

  def prepare_bot(_bot); end
end

class Bot::PriceLimitCheckJobTest < ActiveSupport::TestCase
  include LimitCheckJobBehaviorTests

  private

  def job_class = Bot::PriceLimitCheckJob
  def condition_method = :get_price_limit_condition_met?

  def expected_next_check_at(_bot)
    Time.now.utc.end_of_minute
  end
end

class Bot::PriceDropLimitCheckJobTest < ActiveSupport::TestCase
  include LimitCheckJobBehaviorTests

  private

  def job_class = Bot::PriceDropLimitCheckJob
  def condition_method = :get_price_drop_limit_condition_met?

  def expected_next_check_at(_bot)
    Time.now.utc.end_of_minute
  end
end

class Bot::MovingAverageLimitCheckJobTest < ActiveSupport::TestCase
  include LimitCheckJobBehaviorTests

  private

  def job_class = Bot::MovingAverageLimitCheckJob
  def condition_method = :get_moving_average_limit_condition_met?

  def prepare_bot(bot)
    bot.stubs(:moving_average_limit_in_timeframe_duration).returns(1.hour)
  end

  def expected_next_check_at(bot)
    Time.now.utc + Utilities::Time.seconds_to_current_candle_close(bot.moving_average_limit_in_timeframe_duration)
  end
end

class Bot::IndicatorLimitCheckJobTest < ActiveSupport::TestCase
  include LimitCheckJobBehaviorTests

  private

  def job_class = Bot::IndicatorLimitCheckJob
  def condition_method = :get_indicator_limit_condition_met?

  def prepare_bot(bot)
    bot.stubs(:indicator_limit_in_timeframe_duration).returns(1.hour)
  end

  def expected_next_check_at(bot)
    Time.now.utc + Utilities::Time.seconds_to_current_candle_close(bot.indicator_limit_in_timeframe_duration)
  end
end
