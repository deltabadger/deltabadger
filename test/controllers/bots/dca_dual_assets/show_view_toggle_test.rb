require 'test_helper'

# Renders the dual-asset bot show page and asserts the slider <-> donut view toggle.
#
# Unlike the index bot (whose preview rows are read-only, and whose sliders are CSS-hidden
# under `.bot-locked` while running), the dual-asset barbell slider is *draggable*. A
# click-to-toggle interaction would fight dragging, so the donut + toggle scaffolding is
# rendered ONLY when the bot is running (`working?`) — where the slider is `disabled`, so
# toggling is conflict-free. A stopped/created bot shows the interactive barbell slider
# alone: no donut, no toggle. The generic `donut-chart` controller is reused with
# server-computed slices and no `storageKey` (default pie, session-only toggle).
class Bots::DcaDualAssets::ShowViewToggleTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
  end

  test 'running bot renders the donut-chart pie/list toggle scaffolding' do
    bot = create(:dca_dual_asset, :waiting, user: @user)

    get bot_path(id: bot.id)

    assert_response :success
    assert_select '[data-controller~="donut-chart"]'
    assert_select '[data-donut-chart-target~="pie"][data-action*="donut-chart#toggle"]'
    assert_select '[data-donut-chart-target~="svg"]'
    # The `list` target is the whole `.barbell` (so toggling hides the plates + bar together),
    # not the inner `.slider`.
    assert_select '.barbell[data-donut-chart-target~="list"][data-action*="donut-chart#toggle"]'
    assert_select '.slider[data-donut-chart-target~="list"]', false
  end

  test 'running bot donut data carries both asset symbols' do
    bot = create(:dca_dual_asset, :waiting, user: @user)

    get bot_path(id: bot.id)

    assert_response :success
    node = css_select('[data-controller~="donut-chart"]').first
    data = node && node['data-donut-chart-data-value']
    assert data.present?, 'expected data-donut-chart-data-value on the donut-chart element'
    assert_includes data, bot.base0_asset.symbol
    assert_includes data, bot.base1_asset.symbol
  end

  test 'running bot disables the allocation slider' do
    bot = create(:dca_dual_asset, :waiting, user: @user)

    get bot_path(id: bot.id)

    assert_response :success
    assert_select 'input[data-bot--barbell-allocation-target="allocation0"][disabled]'
  end

  test 'stopped bot shows the slider only — no donut, no toggle' do
    bot = create(:dca_dual_asset, :stopped, user: @user)

    get bot_path(id: bot.id)

    assert_response :success
    assert_select '.barbell'
    assert_select '[data-controller~="donut-chart"]', false
    assert_select '[data-donut-chart-target~="pie"]', false
    assert_select '[data-action*="donut-chart#toggle"]', false
    # Slider is editable when stopped (and not market-cap allocated — the default
    # bitcoin/ethereum factory assets have no market_cap, so `marketcap_allocated?` is false).
    assert_select 'input[data-bot--barbell-allocation-target="allocation0"]:not([disabled])'
  end
end
