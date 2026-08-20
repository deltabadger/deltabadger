require 'test_helper'

# The rebalance trigger is condition-driven, not schedule-driven: it asks "how far has the
# portfolio drifted from its target split" and nothing about the clock. These pin the condition
# itself — placement lives in the order-setter tests.
class Bot::RebalanceableTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_dual_asset, user: create(:user))
    enable_rebalancing(threshold: 0.05)
  end

  test 'drift below the threshold is not due' do
    stub_values(base0: 52, base1: 48) # 52 % vs a 50 % target → 2 points of drift

    assert_in_delta 0.02, @bot.rebalance_drift, 0.0001
    assert_not @bot.rebalance_due?
  end

  test 'drift above the threshold is due' do
    stub_values(base0: 70, base1: 30)

    assert_in_delta 0.20, @bot.rebalance_drift, 0.0001
    assert @bot.rebalance_due?
  end

  test 'drift is symmetric — an underweight base0 triggers exactly like an overweight one' do
    stub_values(base0: 30, base1: 70)

    assert_in_delta 0.20, @bot.rebalance_drift, 0.0001
    assert @bot.rebalance_due?
  end

  test 'drift is measured against a non-even target allocation' do
    update_settings(allocation0: 0.8)
    stub_values(base0: 80, base1: 20)

    assert_in_delta 0.0, @bot.rebalance_drift, 0.0001
    assert_not @bot.rebalance_due?
  end

  test 'a bot with nothing accumulated is not due and makes no exchange call' do
    stub_values(base0: 0, base1: 0)

    assert_nil @bot.rebalance_drift
    assert_not @bot.rebalance_due?
  end

  test 'a disabled bot is never due, however far it has drifted' do
    update_settings(rebalance_enabled: false)
    stub_values(base0: 90, base1: 10)

    assert_not @bot.rebalance_due?
  end

  test 'stale prices are never rebalanced on' do
    # A carried-over price from the last trade can be weeks old (rejected key, dead venue). Trading
    # real money against it is worse than waiting for the next cycle.
    stub_values(base0: 90, base1: 10, stale: true)

    assert_not @bot.rebalance_due?
  end

  test 'a pending rebalance blocks a new one — resume only' do
    stub_values(base0: 70, base1: 30)
    @bot.set_rebalance_pending!(phase: 'selling')

    assert_not @bot.rebalance_due?, 'a second sell while one is in flight is the churn loop'
  end

  test 'the threshold is entered as a percentage and stored as a fraction' do
    parsed = @bot.parse_params(rebalance_enabled: '1', rebalance_threshold: '5')

    assert_in_delta 0.05, parsed[:rebalance_threshold], 0.0001
  end

  test 'a bot saved before this feature reads defaults without dirtying settings' do
    # An existing row has no rebalance_* keys. A persisted default would trip
    # Accountable#check_missed_quote_amount_was_set on the next routine save.
    legacy = @bot
    legacy.update_columns(settings: legacy.settings.except('rebalance_enabled', 'rebalance_threshold'))
    legacy.reload

    assert_not legacy.rebalance_enabled?
    assert_not legacy.rebalance_due?
    assert_equal Bot::Rebalanceable::DEFAULT_THRESHOLD, legacy.rebalance_threshold
    # The contract is that the FALLBACKS are not written back. (settings_changed? is already true on
    # any load — SmartIntervalable's after_initialize writes a default of its own — so asserting on
    # it would test that unrelated behaviour instead of this one.)
    assert_not legacy.settings.key?('rebalance_enabled')
    assert_not legacy.settings.key?('rebalance_threshold')
  end

  private

  # Any settings write has to re-set the carry first — Accountable refuses to save otherwise.
  def update_settings(**attrs)
    @bot.settings = @bot.settings.merge(attrs.transform_keys(&:to_s))
    @bot.set_missed_quote_amount
    @bot.save!
  end

  def enable_rebalancing(threshold:)
    update_settings(rebalance_enabled: true, rebalance_threshold: threshold)
  end

  # Drift is read off live values, never off cost basis — an asset that doubled is overweight
  # regardless of what it cost.
  def stub_values(base0:, base1:, stale: false)
    @bot.stubs(:metrics_with_current_prices).returns(
      total_base0_amount_value_in_quote: base0.to_d,
      total_base1_amount_value_in_quote: base1.to_d,
      prices_stale: stale
    )
  end
end
