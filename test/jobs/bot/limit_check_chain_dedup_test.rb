require 'test_helper'

# A :waiting limit bot is polled by a single self-rescheduling chain: each run of the check job
# enqueues exactly one successor. The chain is one-in-one-out, so it cannot fork itself — but it
# also cannot heal a fork, and any path that STARTS a second chain (recovery, resume, a manual
# rescue) leaves the bot polling twice a minute forever.
#
# A forked chain is silent: the bot keeps working, it just polls two, three or four times a minute
# forever, multiplying its exchange price reads and every log line it emits. Because no single
# enqueue path can be shown to be the culprit, the poll itself is made self-healing instead:
# whatever creates a duplicate, the next tick collapses it.
class Bot::LimitCheckChainDedupTest < ActiveSupport::TestCase
  setup do
    @bot = create(:dca_single_asset, :started)
    @bot.update!(status: :waiting)
    @bot.stubs(:get_price_drop_limit_condition_met?).returns(Result::Success.new(false))
  end

  def live_job_ids_for(bot)
    global_id = bot.to_global_id.to_s
    [SolidQueue::ScheduledExecution, SolidQueue::ReadyExecution, SolidQueue::BlockedExecution]
      .flat_map do |model|
        model.joins(:job)
             .where(solid_queue_jobs: { class_name: 'Bot::PriceDropLimitCheckJob' })
             .select { |e| e.job.arguments.to_s.include?(global_id) }
             .map(&:job_id)
      end
  end

  def chains_for(bot)
    live_job_ids_for(bot).size
  end

  test 'a forked chain collapses to one on the next poll' do
    2.times { Bot::PriceDropLimitCheckJob.set(wait_until: 1.minute.from_now).perform_later(@bot) }
    assert_equal 2, chains_for(@bot), 'setup should model a forked chain'

    Bot::PriceDropLimitCheckJob.new.perform(@bot)

    assert_equal 1, chains_for(@bot)
  end

  test 'a four-way fork collapses to one, not to two' do
    4.times { Bot::PriceDropLimitCheckJob.set(wait_until: 1.minute.from_now).perform_later(@bot) }

    Bot::PriceDropLimitCheckJob.new.perform(@bot)

    assert_equal 1, chains_for(@bot)
  end

  # The prune keeps the NEWEST live job rather than sparing the one this run enqueued. That is what
  # makes it safe when two chains race: each would otherwise spare its own successor and destroy
  # the other's, leaving the bot with none. Keeping the newest is order-independent, so every
  # interleaving converges on one and none can reach zero.
  test 'the surviving chain is the newest job' do
    2.times { Bot::PriceDropLimitCheckJob.set(wait_until: 1.minute.from_now).perform_later(@bot) }
    before = live_job_ids_for(@bot)

    Bot::PriceDropLimitCheckJob.new.perform(@bot)

    surviving = live_job_ids_for(@bot)
    assert_equal 1, surviving.size
    assert_operator surviving.first, :>, before.max, "the newest job (this run's successor) survives"
  end

  # The failure mode that matters more than the fork: a :waiting bot with NO chain never polls
  # again, so it is wedged until Bot::RepairOrphanedBotsJob notices. Pruning must never end empty.
  test 'a healthy single chain still leaves exactly one chain' do
    Bot::PriceDropLimitCheckJob.set(wait_until: 1.minute.from_now).perform_later(@bot)

    Bot::PriceDropLimitCheckJob.new.perform(@bot)

    assert_equal 1, chains_for(@bot)
  end

  test 'a poll with no existing chain still arms one' do
    assert_equal 0, chains_for(@bot)

    Bot::PriceDropLimitCheckJob.new.perform(@bot)

    assert_equal 1, chains_for(@bot)
  end

  # prune_duplicate_solid_queue_jobs selects by job CLASS across the whole queue and narrows to the
  # record, so a prune that got that filter wrong would silently stop every other bot in the
  # installation from polling. That is a far worse bug than the one being fixed.
  test 'pruning never touches another bot chain' do
    other = create(:dca_single_asset, :started, exchange: @bot.exchange,
                                                base_asset: @bot.base_asset, quote_asset: @bot.quote_asset)
    other.update!(status: :waiting)
    2.times { Bot::PriceDropLimitCheckJob.set(wait_until: 1.minute.from_now).perform_later(other) }
    Bot::PriceDropLimitCheckJob.set(wait_until: 1.minute.from_now).perform_later(@bot)

    Bot::PriceDropLimitCheckJob.new.perform(@bot)

    assert_equal 1, chains_for(@bot)
    assert_equal 2, chains_for(other), "another bot's chains are not this poll's business"
  end

  # The transient path enqueues its successor from a different branch, so it needs the same
  # treatment — and a transient read failure is the common way a poll ends, so it is the branch
  # most likely to be the one perpetuating a fork.
  test 'the transient-failure reschedule also collapses a fork' do
    @bot.unstub(:get_price_drop_limit_condition_met?)
    @bot.stubs(:get_price_drop_limit_condition_met?).raises(Client::TransientNetworkError, 'timeout')
    2.times { Bot::PriceDropLimitCheckJob.set(wait_until: 1.minute.from_now).perform_later(@bot) }

    assert_nothing_raised { Bot::PriceDropLimitCheckJob.new.perform(@bot) }

    assert_equal 1, chains_for(@bot)
    assert_equal 'waiting', @bot.reload.status
  end

  # A bot that has left :waiting returns at line 1 and must not be given a chain, forked or not.
  test 'a bot that is no longer waiting is left alone' do
    Bot::PriceDropLimitCheckJob.set(wait_until: 1.minute.from_now).perform_later(@bot)
    @bot.update!(status: :stopped)

    Bot::PriceDropLimitCheckJob.new.perform(@bot)

    assert_equal 1, chains_for(@bot), 'no prune and no enqueue for a non-waiting bot'
  end
end
