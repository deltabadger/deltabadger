require 'test_helper'

# Clearing a halted liquidation. Everything here is about a clear that must NOT happen: the user
# attested about one event, and the job has to be sure that is the event it is clearing.
class Bot::ResolveLiquidationJobTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_index, user: create(:user), with_api_key: true)
    # The widget repaint is covered where it belongs; nothing here is about a live price read.
    @bot.stubs(:broadcast_metrics_update)
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

  test 'attesting about the listed orders accounts for them' do
    order = abandoned_order('CCC')

    Bot::ResolveLiquidationJob.new.perform(@bot, intent_id: 'none', order_ids: [order.id])

    assert_not_predicate @bot.reload, :liquidation_halted?
  end

  test 'an order the page never listed goes on blocking' do
    # The whole reason the ids travel with the click: an order the venue gave up on after that
    # render was not part of the question, so the answer cannot cover it.
    listed = abandoned_order('CCC')
    later = abandoned_order('DDD')

    Bot::ResolveLiquidationJob.new.perform(@bot, intent_id: 'none', order_ids: [listed.id])

    assert_predicate @bot.reload, :liquidation_halted?
    assert_equal %w[DDD], @bot.halted_liquidation_bases
    assert_equal later.base, 'DDD'
  end

  test 'clearing the intent does not account for an abandoned order beside it' do
    # One attestation, two kinds of uncertainty. Clearing the intent must not lift the block for a
    # sale it never covered.
    abandoned_order('CCC')
    @bot.start_liquidation_placement!('DDD')
    @bot.flag_liquidation_ambiguous!

    Bot::ResolveLiquidationJob.new.perform(@bot, intent_id: @bot.liquidation_pending[:id])

    assert_not_predicate @bot.reload, :liquidation_pending?
    assert_predicate @bot, :liquidation_halted?
    assert_equal %w[CCC], @bot.halted_liquidation_bases
  end

  test 'the clear is recorded in the activity log' do
    @bot.start_liquidation_placement!('CCC')
    @bot.flag_liquidation_ambiguous!

    Bot::ResolveLiquidationJob.new.perform(@bot, intent_id: @bot.liquidation_pending[:id],
                                                 user_id: @bot.user_id)

    assert @bot.bot_activity_logs.exists?(event: 'liquidation_manually_resolved')
  end

  private

  # An order the venue stopped reporting: placed, then abandoned by Bot::StaleOrderResolver.
  def abandoned_order(base)
    # The row's own broadcast wants a ticker for the base; nothing here is about that.
    Bot.any_instance.stubs(:broadcast_new_order)
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                         external_status: :abandoned, external_id: "gone-#{base}", side: :sell, base: base,
                         quote: @bot.quote_asset.symbol, transaction_type: 'LIQUIDATION', price: 100, amount: 1)
  end
end
