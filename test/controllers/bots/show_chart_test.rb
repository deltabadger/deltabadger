# frozen_string_literal: true

require 'test_helper'

# The chart summary (date, PnL in quote currency, PnL in %) is rendered by the Stimulus
# controller from the series it gets in the data attributes, so the view's job is to hand
# over the flat labels/series pair and the targets to write into.
class Bots::ShowChartTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true) # satisfies the onboarding gate
    @user = create(:user)
    @bot = create(:dca_index, user: @user)
    sign_in @user

    @store = ActiveSupport::Cache::MemoryStore.new
    Rails.stubs(:cache).returns(@store)
  end

  test 'chart hands the series and summary targets to the controller' do
    data = @bot.metrics.deep_dup
    data[:chart][:labels] = [Time.utc(2026, 1, 1), Time.utc(2026, 2, 1)]
    data[:chart][:series] = [[100.0, 260.0], [100.0, 200.0]]
    Rails.cache.write("bot_#{@bot.id}_metrics_with_current_prices_and_candles", data)

    get bot_path(id: @bot.id)

    assert_response :success
    assert_select '#chart [data-controller="bot--chart"]', 1 do |chart|
      assert_equal [[100.0, 260.0], [100.0, 200.0]],
                   JSON.parse(chart.first['data-bot--chart-series-value'])
      assert_equal 2, JSON.parse(chart.first['data-bot--chart-labels-value']).size
      assert_equal @bot.quote_asset.symbol, chart.first['data-bot--chart-quote-value']
    end
    %w[summary date pnl percent analyzerChart].each do |target|
      assert_select %([data-bot--chart-target="#{target}"]), 1
    end
  end

  # PnL is derived server-side from the two series the chart already draws: how far ahead of
  # what was put in, at every point, in the quote currency. Here 200 invested is worth 260, so
  # the curve ends at +60 — the same number the headline shows, which is why the switch needs
  # no caption. Absolute, so a deposit moves both terms together and does not move the curve.
  test 'chart hands over the pnl curve alongside the value one' do
    data = @bot.metrics.deep_dup
    data[:chart][:labels] = [Time.utc(2026, 1, 1), Time.utc(2026, 2, 1)]
    data[:chart][:series] = [[100.0, 260.0], [100.0, 200.0]]
    Rails.cache.write("bot_#{@bot.id}_metrics_with_current_prices_and_candles", data)

    get bot_path(id: @bot.id)

    assert_select '#chart [data-controller="bot--chart"]', 1 do |chart|
      assert_equal [0.0, 60.0], JSON.parse(chart.first['data-bot--chart-pnl-value'])
    end
  end

  # Nothing invested, nothing to be ahead or behind of. A switch with one usable side is
  # furniture.
  test 'a chart with no pnl curve offers no switch' do
    data = @bot.metrics.deep_dup
    data[:chart][:labels] = [Time.utc(2026, 1, 1), Time.utc(2026, 2, 1)]
    data[:chart][:series] = [[0.0, 0.0], [0.0, 0.0]]
    Rails.cache.write("bot_#{@bot.id}_metrics_with_current_prices_and_candles", data)

    get bot_path(id: @bot.id)

    assert_response :success
    assert_select '#chart [data-controller="bot--chart"]', 1
    assert_select '#chart [role="radiogroup"]', 0
  end

  # A radiogroup, not two independently-pressable toggles: the chart is in one mode at a time.
  # VALUE is the one showing, so it is the one checked and the only one in the tab order.
  test 'chart offers the value/return switch with value selected' do
    data = @bot.metrics.deep_dup
    data[:chart][:labels] = [Time.utc(2026, 1, 1), Time.utc(2026, 2, 1)]
    data[:chart][:series] = [[100.0, 260.0], [100.0, 200.0]]
    Rails.cache.write("bot_#{@bot.id}_metrics_with_current_prices_and_candles", data)

    get bot_path(id: @bot.id)

    assert_select '#chart [role="radiogroup"]', 1
    assert_select '#chart [role="radio"]', 2
    assert_select '#chart [role="radio"][data-value="value"][aria-checked="true"][tabindex="0"]', 1
    assert_select '#chart [role="radio"][data-value="pnl"][aria-checked="false"][tabindex="-1"]', 1
    # The shared control owns the chip and the aria; the chart only listens for the choice.
    assert_select '#chart [data-action="segmented:change->bot--chart#mode"]', 1
    assert_select '#chart .segmented--fluid', 0, 'the two labels are near enough in width to share a column'
  end

  test 'chart shows the placeholder while metrics are still loading' do
    get bot_path(id: @bot.id)

    assert_response :success
    assert_select '#chart [data-controller="bot--chart"]', 0
    assert_select '#chart [role="radiogroup"]', 0
    assert_select '#chart .widget--chart__plot .loader', 1
  end
end
