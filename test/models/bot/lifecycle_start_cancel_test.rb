require 'test_helper'

# Bot::Lifecycle#start used to enqueue a Bot::ActionJob without cancelling one that was already
# scheduled, leaving TWO live job chains for a single bot. Every other re-enqueue site cancels
# first — Bot::RepairOrphanedBotsJob#repair_bot (repair_orphaned_bots_job.rb:67), Bot::Reversible
# (reversible.rb:116), and #stop itself (lifecycle.rb:68). #start was the outlier.
#
# Why it costs money: start_fresh also RESETS started_at, which is the anchor
# Automation::Schedulable#next_interval_checkpoint_at computes the grid from. Both surviving chains
# then reschedule off the SAME new anchor and converge on the SAME grid point, so the bot places
# twice, seconds apart. Observed in production on a daily bot that had traded cleanly at 13:52 for
# a week, was restarted at 10:00:40, and then placed two orders 26s apart at 10:00:43 the next day.
#
# Bot::ActionJob#perform's `raise if next_action_job_at.present?` guard is only a partial net: it
# sees ScheduledExecution only, so it misses a sibling already promoted to Ready/Claimed, and when
# it does fire it dead-letters the job — a silent failure rather than a fix.
class Bot::LifecycleStartCancelTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper

  setup do
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
  end

  test 'start cancels an already-scheduled action job before enqueueing' do
    bot = create(:dca_single_asset)
    Bot::ActionJob.stubs(:perform_later)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))

    bot.expects(:cancel_scheduled_action_jobs).at_least_once

    bot.start
  end

  # Ordering is load-bearing: cancelling AFTER the enqueue would destroy the job just created.
  test 'the cancel happens before the new job is enqueued' do
    bot = create(:dca_single_asset)
    order = sequence('start_order')
    bot.expects(:cancel_scheduled_action_jobs).in_sequence(order)
    Bot::ActionJob.expects(:perform_later).in_sequence(order)

    bot.start
  end

  test 'a restart that reschedules rather than trading now also cancels first' do
    bot = create(:dca_single_asset, :stopped)
    bot.stubs(:restarting_within_interval?).returns(true)
    order = sequence('restart_order')
    bot.expects(:cancel_scheduled_action_jobs).in_sequence(order)
    Bot::ActionJob.expects(:set).in_sequence(order).returns(stub(perform_later: true))

    bot.start(start_fresh: false)
  end

  # A start that fails validation must leave the bot's chain as it was: a failed start changes
  # nothing. (A working bot never gets this far; its start is refused first.)
  test 'a start that fails validation does not cancel the existing chain' do
    bot = create(:dca_single_asset, :stopped)
    bot.stubs(:valid?).returns(false)
    bot.expects(:cancel_scheduled_action_jobs).never
    Bot::ActionJob.expects(:perform_later).never

    refute bot.start
  end

  # The end-to-end property that actually matters: one bot, one live chain.
  test 'starting a bot that already has a scheduled job leaves exactly one action job' do
    bot = create(:dca_single_asset, :stopped)
    SolidQueue::Job.where(class_name: 'Bot::ActionJob').destroy_all
    Bot::ActionJob.set(wait_until: 1.day.from_now).perform_later(bot)
    assert_equal 1, action_jobs_for(bot).count, 'precondition: one scheduled job'

    bot.start(start_fresh: false)

    assert_equal 1, action_jobs_for(bot).count,
                 'a restart must not leave two live chains — they converge on one grid point ' \
                 'and place twice'
  end

  test 'a Start superseded after commit cannot cancel the newer job' do
    bot = create(:dca_single_asset, :stopped)
    newer = nil
    bot.define_singleton_method(:cancel_scheduled_action_jobs) do
      unless newer
        fresh = Bot.find(id)
        fresh.stop
        fresh.start(start_fresh: false)
        newer = fresh.run_identity
      end
      super()
    end

    assert bot.start
    jobs = action_jobs_for(bot)
    assert jobs.any? { |job| job.arguments['arguments'][1] == newer }, 'new generation keeps its execution chain'
  end

  private

  def action_jobs_for(bot)
    gid = bot.to_global_id.to_s
    SolidQueue::Job.where(class_name: 'Bot::ActionJob').where(finished_at: nil).select do |job|
      job.arguments.to_s.include?(gid)
    end
  end
end
