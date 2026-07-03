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

  # == persistence: the seeded split must survive the JSON round-trip as a number ==
  # sell_amount reads back as a BigDecimal, so the seed's `sell_amount / 10` is a BigDecimal.
  # A BigDecimal serializes into the JSON settings column as a STRING (Rails preserves precision),
  # so on reload the split is "0.2" — and Float math against it (effective_interval_duration) then
  # raises `String can't be coerced into Float`. The seed must therefore store a plain number.
  # NOTE: the value is still a BigDecimal in memory right after seeding; the bug only appears after
  # a reload, so this test must round-trip through the database.

  test 'seeding the sell base split persists it as a number, not a json string' do
    bot = create(:dca_single_asset) # default interval 'day' => sell_amount/10 dominates the seed
    bot.direction = 'selling'
    bot.sell_amount = 2.0 # base split still blank => before_validation seeds it
    bot.smart_intervaled = true
    bot.set_missed_quote_amount
    bot.save!

    stored = bot.reload.settings['smart_interval_base_amount']
    assert stored.present?, 'the base split should have been seeded'
    assert_not_kind_of String, stored,
                       'the seeded base split must persist as a number, not a JSON string'
  end

  test 'effective_interval_duration tolerates a legacy string sell base split' do
    bot = create(:dca_single_asset)
    bot.direction = 'selling'
    bot.sell_amount = 2.0
    bot.smart_intervaled = true
    bot.smart_interval_base_amount = 0.2
    bot.set_missed_quote_amount
    bot.save!
    # Simulate a row persisted before the fix, whose split was stored as a JSON string.
    bot.update_column(:settings, bot.settings.merge('smart_interval_base_amount' => '0.2'))

    assert_nothing_raised { bot.reload.effective_interval_duration }
    assert_kind_of ActiveSupport::Duration, bot.reload.effective_interval_duration
  end

  test 'effective_interval_duration tolerates a legacy string quote split while buying' do
    bot = create(:dca_single_asset, :started)
    bot.smart_intervaled = true
    bot.smart_interval_quote_amount = 10.0
    bot.set_missed_quote_amount
    bot.save!
    # Mirror of the sell-side legacy row: quote split stored as a JSON string.
    bot.update_column(:settings, bot.settings.merge('smart_interval_quote_amount' => '10.0'))

    assert_nothing_raised { bot.reload.effective_interval_duration }
    assert_kind_of ActiveSupport::Duration, bot.reload.effective_interval_duration
  end
end
