require 'test_helper'

# Deploy-safety: a bot created BEFORE the reverse-into-selling feature has no sell_* / base_amount_*
# keys in its settings JSON. Loading it under the new code must NOT write those defaults into
# settings (that would dirty settings and trip Accountable#check_missed_quote_amount_was_set on the
# next routine save — e.g. Bot::ActionJob's update!(last_action_job_at:)), and it must still validate.
# The sell-side defaults are therefore read-time fallbacks, never persisted-on-load.
class Bot::SellSettingsDefaultsTest < ActiveSupport::TestCase
  SELL_KEYS_PREFIXES = %w[sell_ base_amount_ smart_interval_base_amount direction].freeze

  # Simulate a bot created BEFORE reverse-into-selling: it has all the OLD buy-side keys persisted but
  # none of the NEW sell-side keys. First reload+save so the buy-side seed (smart_interval_quote_amount,
  # only seeded once quote_amount is present) is persisted like a real pre-feature bot, THEN strip the
  # sell-side keys and reload fresh.
  def legacy_bot(factory)
    bot = create(factory, :started)
    bot = bot.class.find(bot.id) # re-run after_initialize with quote_amount present → seeds buy defaults
    bot.set_missed_quote_amount
    bot.save! # persist the buy-side seed a real existing bot would already have
    legacy = bot.reload.settings.reject { |k, _| SELL_KEYS_PREFIXES.any? { |p| k.to_s.start_with?(p) } }
    bot.update_columns(settings: legacy)
    bot.class.find(bot.id)
  end

  test 'loading a legacy single-asset bot does not dirty settings' do
    bot = legacy_bot(:dca_single_asset)
    assert_not bot.will_save_change_to_settings?, 'reading sell defaults must not mark settings dirty'
  end

  test 'a legacy single-asset bot survives a routine status-only save (no wedge)' do
    bot = legacy_bot(:dca_single_asset)
    assert_nothing_raised { bot.update!(last_action_job_at: Time.current) }
  end

  test 'a legacy single-asset bot is still valid' do
    bot = legacy_bot(:dca_single_asset)
    assert bot.valid?, bot.errors.full_messages.to_sentence
  end

  test 'a legacy dual-asset bot survives a routine save (shared sell concerns)' do
    bot = legacy_bot(:dca_dual_asset)
    assert_not bot.will_save_change_to_settings?
    assert_nothing_raised { bot.update!(last_action_job_at: Time.current) }
    assert bot.valid?, bot.errors.full_messages.to_sentence
  end

  # The read-time fallbacks still return the same sensible defaults a fresh bot would show.
  test 'sell defaults still read through as fallbacks' do
    bot = legacy_bot(:dca_single_asset)
    assert_equal false, bot.sell_price_limited
    assert_equal 'above', bot.sell_price_limit_value_condition
    assert_equal 'above', bot.sell_indicator_limit_value_condition
    assert_equal 70, bot.sell_indicator_limit
    assert_equal 'twenty_four_hours', bot.sell_price_drop_limit_time_window_condition
    assert_equal 9, bot.sell_moving_average_limit_in_period
    assert_equal false, bot.base_amount_limited
  end
end
