require 'test_helper'

# Issue #3 — the buy-side "Don't spend more than N quote" cap, inverted for selling:
# "Don't sell more than N base". Mirrors QuoteAmountLimitable but denominated in base and
# accounted from sell executions; enforced via sellable_base_amount and a stop at the cap.
class Bot::BaseAmountLimitableTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

  # Mutate in memory, set_missed, single save! — the Accountable guard requires set_missed before
  # each settings-changing save, so the established pattern is one save per setup.
  def selling_capped_bot(limit:, sell_amount: nil)
    bot = create(:dca_single_asset, :started)
    bot.direction = 'selling'
    bot.base_amount_limited = true
    bot.base_amount_limit = limit
    bot.sell_amount = sell_amount if sell_amount
    bot.set_missed_quote_amount
    bot.save!
    bot.reload
    bot
  end

  # == accounting: closed + open sells both count (no overselling while fills lag) ==

  test 'available-before-limit counts closed and open sells alike' do
    bot = selling_capped_bot(limit: 1.0)
    enabled_at = bot.base_amount_limit_enabled_at
    assert_not_nil enabled_at

    # a filled sell of 0.4 base
    create(:transaction, bot: bot, side: :sell, status: :submitted, external_status: :closed,
                         external_id: 'c1', amount: 0.4, amount_exec: 0.4,
                         created_at: enabled_at + 1.second)
    # an open (unconfirmed) sell of 0.3 base — must still reserve against the cap
    create(:transaction, bot: bot, side: :sell, status: :submitted, external_status: :unknown,
                         external_id: 'o1', amount: 0.3, amount_exec: nil,
                         created_at: enabled_at + 2.seconds)
    bot.reload

    assert_in_delta 0.3, bot.base_amount_available_before_limit_reached, 1e-9
  end

  # == enforcement: the per-tick sell is clamped to the remaining allowance ==

  test 'sellable_base_amount never exceeds the remaining base allowance' do
    bot = selling_capped_bot(limit: 1.0, sell_amount: 5) # user wants to sell a lot
    bot.stubs(:live_free_base_balance).returns(10.to_d)

    # already sold 0.7 → only 0.3 of headroom remains
    create(:transaction, bot: bot, side: :sell, status: :submitted, external_status: :closed,
                         external_id: 'c1', amount: 0.7, amount_exec: 0.7,
                         created_at: bot.base_amount_limit_enabled_at + 1.second)

    assert_in_delta 0.3, bot.send(:sellable_base_amount), 1e-9
  end

  test 'a closed sell with nil amount_exec still counts against the cap (via requested amount)' do
    # Consistent with total_amount / metrics: a closed row that never backfilled amount_exec falls
    # back to the requested amount, so a nil-exec close can't silently regain cap allowance.
    bot = selling_capped_bot(limit: 1.0)
    create(:transaction, bot: bot, side: :sell, status: :submitted, external_status: :closed,
                         external_id: 'c1', amount: 0.6, amount_exec: nil,
                         created_at: bot.base_amount_limit_enabled_at + 1.second)
    bot.reload

    assert_in_delta 0.4, bot.base_amount_available_before_limit_reached, 1e-9
  end

  test 'a sell that closes with nil amount_exec and hits the cap triggers the stop' do
    bot = selling_capped_bot(limit: 1.0)
    order = create(:transaction, bot: bot, side: :sell, status: :submitted, external_status: :open,
                                 external_id: 'o1', amount: 1.0, amount_exec: nil,
                                 created_at: bot.base_amount_limit_enabled_at + 1.second)
    Bot::StopJob.expects(:perform_later).at_least_once

    order.update!(external_status: :closed) # closes with nil exec, reaches the cap
  end

  test 'an exhausted base cap returns 0 without reading the live balance' do
    bot = selling_capped_bot(limit: 1.0, sell_amount: 5)
    create(:transaction, bot: bot, side: :sell, status: :submitted, external_status: :closed,
                         external_id: 'c1', amount: 1.0, amount_exec: 1.0,
                         created_at: bot.base_amount_limit_enabled_at + 1.second)
    bot.expects(:live_free_base_balance).never # cap exhausted — no exchange call

    assert_equal 0, bot.send(:sellable_base_amount)
  end

  # == stop at the cap ==

  test 'a selling bot stops once the base cap is reached' do
    bot = selling_capped_bot(limit: 1.0)
    create(:transaction, bot: bot, side: :sell, status: :submitted, external_status: :closed,
                         external_id: 'c1', amount: 1.0, amount_exec: 1.0,
                         created_at: bot.base_amount_limit_enabled_at + 1.second)
    bot.reload

    assert_predicate bot, :base_amount_limit_reached?
  end

  # == an exhausted cap blocks (re)starting (parity with the quote cap's on: :start guard) ==

  test 'a selling bot whose base cap is already reached cannot be started' do
    bot = selling_capped_bot(limit: 1.0)
    create(:transaction, bot: bot, side: :sell, status: :submitted, external_status: :closed,
                         external_id: 'c1', amount: 1.0, amount_exec: 1.0,
                         created_at: bot.base_amount_limit_enabled_at + 1.second)
    bot.reload

    assert_predicate bot, :base_amount_limit_reached?
    assert_not bot.valid?(:start), 'an exhausted base cap must block (re)starting into a no-op state'
    assert bot.errors.added?(:settings, :base_amount_limit_reached)
  end

  # == the quote cap must NOT fire on sell fills (direction-gated) ==

  test 'a sell fill does not trip the quote spend cap while selling' do
    bot = create(:dca_single_asset, :started)
    bot.direction = 'selling'
    bot.quote_amount_limited = true
    bot.quote_amount_limit = 100
    bot.set_missed_quote_amount
    bot.save!
    bot.reload

    # a sell that nets way more quote than the (buy) limit — must be ignored while selling
    create(:transaction, bot: bot, side: :sell, status: :submitted, external_status: :closed,
                         external_id: 'c1', amount: 1, amount_exec: 1,
                         quote_amount: 500, quote_amount_exec: 500,
                         created_at: bot.quote_amount_limit_enabled_at + 1.second)
    bot.reload

    assert_not bot.quote_amount_limit_reached?, 'the quote cap is a buy-side concept; sells must not count'
  end
end
