require 'test_helper'

# Unit tests for the wizard step-order value object. It is a pure PORO: no DB,
# no routes — it only sequences the existing step keys per (bot_type, variant)
# and derives progress %, owned session keys, and the downstream reset set.
class Bots::Wizard::StepOrderTest < ActiveSupport::TestCase
  SO = Bots::Wizard::StepOrder

  EXCHANGE = SO::EXCHANGE_KEY
  BASE     = SO::BASE_KEY
  BASE0    = SO::BASE0_KEY
  BASE1    = SO::BASE1_KEY
  QUOTE    = SO::QUOTE_KEY

  # ── steps / first / first? ──────────────────────────────────────────────

  test 'single asset_first sequences asset → exchange → api → spendable' do
    order = SO.for(bot_type: :single, variant: :asset_first)
    assert_equal %i[currencies exchange api spendable], order.steps
    assert_equal :currencies, order.first
    assert order.first?(:currencies)
    refute order.first?(:exchange)
  end

  test 'single exchange_first reverses to exchange → api → asset → spendable' do
    order = SO.for(bot_type: :single, variant: :exchange_first)
    assert_equal %i[exchange api currencies spendable], order.steps
    assert_equal :exchange, order.first
    assert order.first?(:exchange)
    refute order.first?(:currencies)
  end

  test 'dual asset_first sequences asset → asset2 → exchange → api → spendable' do
    order = SO.for(bot_type: :dual, variant: :asset_first)
    assert_equal %i[currencies currencies2 exchange api spendable], order.steps
    assert_equal :currencies, order.first
  end

  test 'dual exchange_first sequences exchange → api → asset → asset2 → spendable' do
    order = SO.for(bot_type: :dual, variant: :exchange_first)
    assert_equal %i[exchange api currencies currencies2 spendable], order.steps
    assert_equal :exchange, order.first
  end

  test 'variant defaults to asset_first when omitted' do
    assert_equal SO.for(bot_type: :single, variant: :asset_first).steps,
                 SO.for(bot_type: :single).steps
  end

  # ── next_after ──────────────────────────────────────────────────────────

  test 'next_after returns the following step and nil at the end' do
    order = SO.for(bot_type: :single, variant: :asset_first)
    assert_equal :exchange, order.next_after(:currencies)
    assert_equal :api, order.next_after(:exchange)
    assert_equal :spendable, order.next_after(:api)
    assert_nil order.next_after(:spendable)
  end

  test 'next_after follows the reversed order in exchange_first' do
    order = SO.for(bot_type: :single, variant: :exchange_first)
    assert_equal :api, order.next_after(:exchange)
    assert_equal :currencies, order.next_after(:api)
    assert_equal :spendable, order.next_after(:currencies)
    assert_nil order.next_after(:spendable)
  end

  # ── progress (index-based even spacing) ─────────────────────────────────

  test 'progress is index-based even spacing for a 4-step single flow' do
    order = SO.for(bot_type: :single, variant: :asset_first)
    assert_equal 25, order.progress(:currencies)
    assert_equal 50, order.progress(:exchange)
    assert_equal 75, order.progress(:api)
    assert_equal 100, order.progress(:spendable)
  end

  test 'progress is index-based even spacing for a 5-step dual flow' do
    order = SO.for(bot_type: :dual, variant: :asset_first)
    assert_equal 20, order.progress(:currencies)
    assert_equal 40, order.progress(:currencies2)
    assert_equal 60, order.progress(:exchange)
    assert_equal 80, order.progress(:api)
    assert_equal 100, order.progress(:spendable)
  end

  test 'progress tracks the reversed order so the first step is always lowest' do
    order = SO.for(bot_type: :single, variant: :exchange_first)
    assert_equal 25, order.progress(:exchange)
    assert_equal 100, order.progress(:spendable)
  end

  # ── owned_keys ──────────────────────────────────────────────────────────

  test 'owned_keys maps each step to the session key it writes' do
    order = SO.for(bot_type: :single, variant: :asset_first)
    assert_equal [BASE], order.owned_keys(:currencies)
    assert_equal [EXCHANGE], order.owned_keys(:exchange)
    assert_equal [QUOTE], order.owned_keys(:spendable)
    assert_equal [], order.owned_keys(:api), 'api key lives in the DB, not the session'
  end

  test 'owned_keys for :currencies is base0 in a dual flow' do
    order = SO.for(bot_type: :dual, variant: :asset_first)
    assert_equal [BASE0], order.owned_keys(:currencies)
    assert_equal [BASE1], order.owned_keys(:currencies2)
  end

  # ── reset_keys (ALL − keys owned by steps BEFORE the step) ───────────────

  test 'reset_keys for the single first asset is a full wipe (today behavior)' do
    order = SO.for(bot_type: :single, variant: :asset_first)
    assert_equal SO::ALL_WIZARD_KEYS.to_set, order.reset_keys(:currencies).to_set
  end

  test 'reset_keys always includes the stale dual base keys so they cannot leak' do
    order = SO.for(bot_type: :single, variant: :asset_first)
    # Re-picking the single asset must clear base0/base1 left over from a prior
    # dual session (they would otherwise pass through sanitized_bot_config).
    assert_includes order.reset_keys(:currencies), BASE0
    assert_includes order.reset_keys(:currencies), BASE1
  end

  test 'reset_keys for re-picking the exchange keeps the chosen asset (sticky) and clears the quote' do
    # The asset is the anchor and the exchange list is asset-filtered, so an
    # exchange re-pick must NOT discard the asset — in either variant.
    order = SO.for(bot_type: :single, variant: :exchange_first)
    keys = order.reset_keys(:exchange)
    refute_includes keys, BASE, 'the chosen asset survives an exchange re-pick'
    assert_includes keys, EXCHANGE
    assert_includes keys, QUOTE
  end

  test 'reset_keys for re-picking the dual exchange keeps both base assets' do
    order = SO.for(bot_type: :dual, variant: :exchange_first)
    keys = order.reset_keys(:exchange)
    refute_includes keys, BASE0
    refute_includes keys, BASE1
    assert_includes keys, EXCHANGE
    assert_includes keys, QUOTE
  end

  test 'reset_keys for re-picking the asset mid exchange_first keeps the chosen exchange' do
    order = SO.for(bot_type: :single, variant: :exchange_first)
    keys = order.reset_keys(:currencies)
    refute_includes keys, EXCHANGE, 'exchange is owned upstream, must survive'
    assert_includes keys, QUOTE
    assert_includes keys, BASE
  end

  test 'reset_keys for the dual exchange step keeps both base assets, clears quote and stale single base' do
    order = SO.for(bot_type: :dual, variant: :asset_first)
    keys = order.reset_keys(:exchange)
    refute_includes keys, BASE0
    refute_includes keys, BASE1
    assert_includes keys, EXCHANGE
    assert_includes keys, QUOTE
    assert_includes keys, BASE
  end

  test 'reset_keys for the dual second asset (exchange_first) keeps exchange and base0' do
    order = SO.for(bot_type: :dual, variant: :exchange_first)
    keys = order.reset_keys(:currencies2)
    refute_includes keys, EXCHANGE
    refute_includes keys, BASE0
    assert_includes keys, BASE1
    assert_includes keys, QUOTE
  end
end
