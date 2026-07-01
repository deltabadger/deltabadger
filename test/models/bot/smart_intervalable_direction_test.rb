require 'test_helper'

# Issue #5 — Smart Intervals, inverted for selling: split the SELL amount into N-base units at a
# finer cadence (mirror of the buy-side quote split), instead of the current hard bypass that makes
# the row inert while selling.
class Bot::SmartIntervalableDirectionTest < ActiveSupport::TestCase
  # == seeding: entering a sell amount on a smart-on selling bot must not be rejected ==
  # Right after a flip the sell base split is blank (it could not be seeded while sell_amount was
  # blank). When the user first types a sell amount in the MAIN sentence, that form carries no base
  # split — the bot must auto-seed it rather than reject the update for a missing Smart Intervals value.

  test 'setting a sell amount on a smart-on selling bot auto-seeds the base split and validates' do
    bot = create(:dca_single_asset, :started)
    bot.direction = 'selling'
    bot.smart_intervaled = true
    bot.sell_amount = 2.0 # user enters only the sell amount; base split still blank
    bot.set_missed_quote_amount

    assert bot.valid?, bot.errors.full_messages.to_sentence
    assert bot.smart_interval_base_amount.to_d.positive?,
           'the base split is seeded so the user need not first visit the Smart Intervals rule'
  end

  # == flip safety: a smart-on buy bot must flip without a validation error ==

  test 'flipping a smart-intervaled buying bot to selling does not raise' do
    bot = create(:dca_single_asset, :started)
    bot.smart_intervaled = true
    bot.smart_interval_quote_amount = 10.0
    bot.set_missed_quote_amount
    bot.save!

    assert_nothing_raised { bot.flip_direction! }
    assert_predicate bot.reload, :selling?
  end

  # == per-tick sell amount comes from the base split ==

  test 'while selling, the per-tick sell amount is the base split, not the full sell amount' do
    bot = create(:dca_single_asset, :started)
    bot.direction = 'selling'
    bot.sell_amount = 1.0
    bot.smart_intervaled = true
    bot.smart_interval_base_amount = 0.1
    bot.set_missed_quote_amount
    bot.save!
    bot.stubs(:total_amount).returns(10.to_d)
    bot.stubs(:live_free_base_balance).returns(10.to_d)

    assert_in_delta 0.1, bot.send(:sellable_base_amount), 1e-9
  end

  # == cadence subdivides while selling+smart ==

  test 'the sell cadence is subdivided by the base split' do
    bot = create(:dca_single_asset) # not started: sell_interval is editable while inactive
    bot.direction = 'selling'
    bot.sell_interval = 'week'
    bot.sell_amount = 1.0
    bot.smart_intervaled = true
    bot.smart_interval_base_amount = 0.1
    bot.set_missed_quote_amount
    bot.save!

    # 1.0 sold in 0.1 units => 10 sub-intervals => duration is a tenth of a week
    assert_in_delta (1.week / 10).to_f, bot.effective_interval_duration.to_f, 1.0
  end

  # == buying path is untouched ==

  test 'the buying smart-interval cadence is unchanged' do
    bot = create(:dca_single_asset, :started)
    bot.smart_intervaled = true
    bot.smart_interval_quote_amount = 10.0
    bot.set_missed_quote_amount
    bot.save!

    # buy split: quote_amount / 10 per tick => cadence is interval / (quote_amount/10)
    expected = (bot.interval_duration / (bot.quote_amount.to_f / 10.0)).to_f
    assert_in_delta expected, bot.effective_interval_duration.to_f, 1.0
  end
end
