require 'test_helper'

# The manual ⇄ control rotates through THREE states instead of flipping between two:
#   buy → sell N base → sell BASE for N quote → buy
# `direction` stays two-valued; `sell_denomination` is the new axis (how a sell tick is sized).
class Bot::RotationTest < ActiveSupport::TestCase
  # == sell_denomination defaults & validation ==

  test 'a bot sells base-denominated by default' do
    bot = build(:dca_single_asset)
    assert_equal 'base', bot.sell_denomination
  end

  test 'the base default is never written into settings (existing-row safe)' do
    bot = create(:dca_single_asset)
    assert_not bot.settings.key?('sell_denomination')

    bot.reload
    assert_equal 'base', bot.sell_denomination
    assert_not bot.settings.key?('sell_denomination'),
               'reading the default must not write "sell_denomination" into settings'
  end

  test 'rejects an unknown denomination' do
    bot = build(:dca_single_asset)
    bot.sell_denomination = 'sideways'
    assert_not_predicate bot, :valid?
    assert bot.errors[:sell_denomination].present?
  end

  test 'the denomination predicates are false while buying' do
    bot = build(:dca_single_asset)
    assert_not_predicate bot, :sells_base_amount?
    assert_not_predicate bot, :sells_quote_amount?
  end

  test 'the denomination predicates follow direction + denomination' do
    bot = build(:dca_single_asset)
    bot.direction = 'selling'
    assert_predicate bot, :sells_base_amount?
    assert_not_predicate bot, :sells_quote_amount?

    bot.sell_denomination = 'quote'
    assert_not_predicate bot, :sells_base_amount?
    assert_predicate bot, :sells_quote_amount?
  end

  test 'non-reversible bot types answer both predicates with false' do
    # SmartIntervalable and its partials are shared with these types, which never include Reversible.
    bot = build(:dca_dual_asset)
    assert_not_predicate bot, :sells_base_amount?
    assert_not_predicate bot, :sells_quote_amount?
  end

  # == sell_quote_amount ==

  test 'sell_quote_amount is blank by default and rejects a non-positive value' do
    bot = build(:dca_single_asset)
    assert_nil bot.sell_quote_amount

    bot.sell_quote_amount = -5
    assert_not_predicate bot, :valid?
    assert bot.errors[:sell_quote_amount].present?
  end

  test 'sell_quote_amount reads back as a BigDecimal' do
    bot = create(:dca_single_asset)
    bot.sell_quote_amount = 25.5
    bot.set_missed_quote_amount
    bot.save!

    assert_equal 25.5.to_d, bot.reload.sell_quote_amount
  end

  test 'a submitted blank sell_quote_amount clears it' do
    bot = create(:dca_single_asset)
    bot.sell_quote_amount = 25
    bot.set_missed_quote_amount
    bot.save!

    parsed = bot.parse_params(ActionController::Parameters.new(sell_quote_amount: '').permit!)
    assert parsed.key?(:sell_quote_amount)
    assert_nil parsed[:sell_quote_amount]
  end

  test 'a form that does not submit sell_quote_amount leaves it untouched' do
    bot = create(:dca_single_asset)
    parsed = bot.parse_params(ActionController::Parameters.new(interval: 'week').permit!)
    assert_not parsed.key?(:sell_quote_amount)
  end

  # == The rotation itself ==

  test 'rotate_direction! cycles buy → sell base → sell quote → buy' do
    bot = create(:dca_single_asset)
    assert_predicate bot, :buying?

    bot.rotate_direction!
    assert_predicate bot, :sells_base_amount?

    bot.rotate_direction!
    assert_predicate bot, :sells_quote_amount?

    bot.rotate_direction!
    assert_predicate bot, :buying?
    assert_equal 'base', bot.sell_denomination, 'returning to buying resets the denomination'
  end

  test 'the rotation survives a reload (it is persisted, not in-memory)' do
    bot = create(:dca_single_asset)
    bot.rotate_direction!
    bot.rotate_direction!

    assert_predicate bot.reload, :sells_quote_amount?
  end

  test 'a trigger flip never touches the chosen sell denomination' do
    # flip_direction! is the trigger path (start_selling / start_buying) — it must preserve the
    # sell mode the user configured, unlike the manual rotation.
    bot = create(:dca_single_asset)
    bot.rotate_direction!
    bot.rotate_direction!
    assert_predicate bot, :sells_quote_amount?

    bot.flip_direction!
    assert_predicate bot, :buying?
    assert_equal 'quote', bot.sell_denomination

    bot.flip_direction!
    assert_predicate bot, :sells_quote_amount?
  end

  test 'the base → quote step still cancels open orders and reschedules' do
    # It is not a direction change, but the open sell was sized under the old basis.
    bot = create(:dca_single_asset, :started)
    bot.rotate_direction!
    assert_predicate bot.reload, :sells_base_amount?

    order = create(:transaction, bot: bot, side: :sell, status: :submitted, external_status: :open)
    Transaction.any_instance.expects(:cancel).at_least_once.returns(Result::Success.new)

    bot.rotate_direction!

    assert_predicate bot.reload, :sells_quote_amount?
    assert order.present?
  end

  # == Clamping the inactive smart-interval split (pre-existing 500) ==

  test 'entering the buy side clamps a smart quote split that drifted below its minimum' do
    # Only the ACTIVE side's split is validated, so the buy amount can be raised through the API
    # while selling and leave the stored quote split below its (now higher) minimum. Entering the
    # buy side would then raise RecordInvalid and 500 the flip.
    bot = create(:dca_single_asset)
    bot.settings['smart_intervaled'] = true
    bot.settings['smart_interval_quote_amount'] = 10.0
    bot.set_missed_quote_amount
    bot.save!

    bot.rotate_direction!
    assert_predicate bot, :sells_base_amount?

    bot.settings['quote_amount'] = 1_000_000.0 # minimum split now far above 10
    bot.set_missed_quote_amount
    bot.save!(validate: false)

    assert_nothing_raised { bot.rotate_direction! } # → sell/quote
    assert_nothing_raised { bot.rotate_direction! } # → buying, where the split is validated again
    assert_predicate bot, :buying?
    assert_operator bot.smart_interval_quote_amount.to_d, :>=,
                    bot.send(:minimum_smart_interval_quote_amount).to_d
  end

  test 'the clamped split is stored as a number, not a BigDecimal string' do
    # A BigDecimal serialises into the JSON settings column as a String, which breaks the Float
    # division in effective_interval_duration on the next load.
    bot = create(:dca_single_asset)
    bot.settings['smart_intervaled'] = true
    bot.settings['smart_interval_quote_amount'] = 0.01
    bot.settings['quote_amount'] = 1_000_000.0
    bot.set_missed_quote_amount
    bot.save!(validate: false)

    bot.flip_direction!
    bot.flip_direction!

    assert_kind_of Numeric, bot.reload.settings['smart_interval_quote_amount']
  end
end
