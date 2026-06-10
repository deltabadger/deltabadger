# frozen_string_literal: true

require 'test_helper'

# When the combined (candles) cache is cold but the prices cache is warm — the common
# case, since Bot::WarmMetricsCachesJob warms prices every 5 minutes — the balances
# table must render real values immediately; only the chart shows the loading state.
class Bots::ShowMetricsLoadingTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true) # satisfies the onboarding gate
    @user = create(:user)
    @bot = create(:dca_index, user: @user)
    sign_in @user

    @store = ActiveSupport::Cache::MemoryStore.new
    Rails.stubs(:cache).returns(@store)
  end

  test 'balances render from warm prices cache while chart is still loading' do
    prices_data = @bot.metrics.deep_dup
    prices_data[:total_quote_amount_invested] = 1234.56
    prices_data[:asset_values] = {
      'AAPL' => { amount: 1.0, quote_invested: 100.0, current_value: 110.0,
                  current_price: 110.0, avg_price: 100.0, pnl_percentage: 0.1 }
    }
    Rails.cache.write("bot_#{@bot.id}_metrics_with_current_prices", prices_data)

    get bot_path(id: @bot.id)

    assert_response :success
    assert_select '#assets_metrics_table tbody tr', 1 # balances rendered
    # the on-connect trigger must still fire so the chart gets broadcast:
    assert_select '[data-controller="broadcast--on-connect"]', 1
  end

  test 'balances prefer the prices cache over a staler combined cache' do
    # UpdateMetricsJob force-refreshes the prices cache after transaction changes but
    # never the combined (candles) cache, so prices is always at-least-as-fresh.
    stale = @bot.metrics.deep_dup
    stale[:asset_values] = {
      'OLD' => { amount: 1.0, quote_invested: 1.0, current_value: 1.0,
                 current_price: 1.0, avg_price: 1.0, pnl_percentage: 0.0 }
    }
    fresh = stale.deep_dup
    fresh[:asset_values] = stale[:asset_values].merge(
      'NEW' => { amount: 2.0, quote_invested: 2.0, current_value: 2.0,
                 current_price: 1.0, avg_price: 1.0, pnl_percentage: 0.0 }
    )
    Rails.cache.write("bot_#{@bot.id}_metrics_with_current_prices_and_candles", stale)
    Rails.cache.write("bot_#{@bot.id}_metrics_with_current_prices", fresh)

    get bot_path(id: @bot.id)

    assert_response :success
    assert_select '#assets_metrics_table tbody tr', 2 # fresh prices data, not the stale combined
  end

  test 'fully cold caches keep the loading state everywhere' do
    get bot_path(id: @bot.id)

    assert_response :success
    assert_select '#assets_metrics_table tbody tr', 0
    assert_select '[data-controller="broadcast--on-connect"]', 1
  end
end
