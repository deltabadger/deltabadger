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

  test 'fully cold caches keep the loading state everywhere' do
    get bot_path(id: @bot.id)

    assert_response :success
    assert_select '#assets_metrics_table tbody tr', 0
    assert_select '[data-controller="broadcast--on-connect"]', 1
  end
end
