require 'test_helper'

# Clearing a halted liquidation. Everything here is about a clear that must NOT happen: the user
# attested about one event, and the job has to be sure that is the event it is clearing.
class Bot::ResolveLiquidationJobTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_index, user: create(:user), with_api_key: true)
  end

  test 'clears the halt it was raised for' do
    @bot.start_liquidation_placement!('CCC')
    @bot.flag_liquidation_ambiguous!
    intent_id = @bot.liquidation_pending[:id]

    Bot::ResolveLiquidationJob.new.perform(@bot, intent_id: intent_id)

    assert_not_predicate @bot.reload, :liquidation_pending?
  end

  test 'a stale generation id clears nothing' do
    # A resolution queued against an EARLIER halt — a stale tab, a double click — must not wipe an
    # attestation the user never gave for the halt that is actually standing.
    @bot.start_liquidation_placement!('CCC')
    @bot.flag_liquidation_ambiguous!
    stale_id = @bot.liquidation_pending[:id]
    @bot.clear_liquidation_pending!
    @bot.start_liquidation_placement!('DDD')
    @bot.flag_liquidation_ambiguous!

    Bot::ResolveLiquidationJob.new.perform(@bot, intent_id: stale_id)

    assert_predicate @bot.reload, :liquidation_ambiguous?
    assert_equal 'DDD', @bot.liquidation_pending[:symbol]
  end

  test 'a placement still in flight is not resolvable' do
    # The job takes the same semaphore as the placement, so if it sees `placing` the placement has
    # already finished and something else is wrong — but an attestation about an outcome that has
    # not happened yet must never clear it.
    @bot.start_liquidation_placement!('CCC')
    intent_id = @bot.liquidation_pending[:id]

    Bot::ResolveLiquidationJob.new.perform(@bot, intent_id: intent_id)

    assert_predicate @bot.reload, :liquidation_pending?
  end

  test 'no halt at all is a no-op rather than an error' do
    assert_nothing_raised { Bot::ResolveLiquidationJob.new.perform(@bot, intent_id: 'anything') }
  end

  test 'the clear is recorded in the activity log' do
    @bot.start_liquidation_placement!('CCC')
    @bot.flag_liquidation_ambiguous!

    Bot::ResolveLiquidationJob.new.perform(@bot, intent_id: @bot.liquidation_pending[:id],
                                                 user_id: @bot.user_id)

    assert @bot.bot_activity_logs.exists?(event: 'liquidation_manually_resolved')
  end
end
