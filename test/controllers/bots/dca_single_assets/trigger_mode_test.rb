require 'test_helper'

# Issues #1/#2 — the two per-rule dropdowns (action + timing) collapse into one direction-aware
# `…_mode` select. The select submits a synthetic `…_mode` param that must (a) survive strong-params
# permitting and (b) be decoded in parse_params back into the stored (timing_condition, action) pair.
#
#   restrict -> timing=while, action=pause           ("Buy only"  / "Sell only")
#   start    -> timing=after, action=pause           ("Start buying" / "Start selling")
#   flip     -> action=start_selling|start_buying     ("Start selling" / "Start buying")
class Bots::DcaSingleAssets::TriggerModeTest < ActionDispatch::IntegrationTest
  TURBO = { 'Accept' => 'text/vnd.turbo-stream.html, text/html' }.freeze

  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    @bot = create(:dca_single_asset, user: @user) # created (editable) bot, buying
  end

  def patch_settings(attrs)
    patch bot_path(id: @bot.id), params: { bots_dca_single_asset: attrs }, headers: TURBO
    @bot.reload
  end

  # == buy side ==

  test 'restrict mode decodes to while + pause on the buy side' do
    patch_settings(price_limited: 'true', price_limit_mode: 'restrict')

    assert_equal 'while', @bot.price_limit_timing_condition
    assert_equal 'pause', @bot.price_limit_action
  end

  test 'start mode decodes to after + pause on the buy side' do
    patch_settings(price_limited: 'true', price_limit_mode: 'start')

    assert_equal 'after', @bot.price_limit_timing_condition
    assert_equal 'pause', @bot.price_limit_action
  end

  test 'flip mode on a buying bot starts selling' do
    patch_settings(price_limited: 'true', price_limit_mode: 'flip')

    assert_equal 'start_selling', @bot.price_limit_action
    assert_equal 'after', @bot.price_limit_timing_condition
  end

  # == sell side ==

  test 'flip mode on a selling bot starts buying (and the virtual key is permitted)' do
    @bot.flip_direction!
    assert_predicate @bot.reload, :selling?

    patch bot_path(id: @bot.id), params: { bots_dca_single_asset: { sell_price_limited: 'true', sell_price_limit_mode: 'flip' } }, headers: TURBO
    @bot.reload

    assert_equal 'start_buying', @bot.sell_price_limit_action
    assert_equal 'after', @bot.sell_price_limit_timing_condition
  end

  test 'restrict mode on the sell side decodes to while + pause' do
    @bot.flip_direction!

    patch bot_path(id: @bot.id), params: { bots_dca_single_asset: { sell_price_limited: 'true', sell_price_limit_mode: 'restrict' } }, headers: TURBO
    @bot.reload

    assert_equal 'while', @bot.sell_price_limit_timing_condition
    assert_equal 'pause', @bot.sell_price_limit_action
  end

  # == price-drop has no timing; only [start, flip] ==

  test 'price-drop flip mode flips, start mode stays a pause gate' do
    patch_settings(price_drop_limited: 'true', price_drop_limit_mode: 'flip')
    assert_equal 'start_selling', @bot.price_drop_limit_action

    patch_settings(price_drop_limited: 'true', price_drop_limit_mode: 'start')
    assert_equal 'pause', @bot.price_drop_limit_action
  end

  # the virtual key never leaks into persisted settings
  test 'the _mode key is consumed by parse_params and never stored' do
    patch_settings(price_limited: 'true', price_limit_mode: 'flip')

    assert_not @bot.settings.key?('price_limit_mode')
  end
end
