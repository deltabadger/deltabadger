require 'test_helper'

# With history the chart head gets the bot chart controller, fed the same way a bot feeds it:
# labels, [value, invested] series and a P/L series in the display currency.
class TrackerHistoryTest < ActionDispatch::IntegrationTest
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @user = create(:user, admin: true, setup_completed: true)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    @btc = create(:asset, :bitcoin)
    AccountBalance.create!(user: @user, exchange: @binance, asset: @btc, free: 1, locked: 0, usd_price: 40_000,
                           usd_value: 40_000, synced_at: Time.current, priced_at: Time.current)
    create(:account_transaction, api_key: @key, entry_type: :deposit, base_currency: 'USD', base_amount: 30_000,
                                 quote_currency: nil, quote_amount: nil, transacted_at: 3.days.ago)
    Tracker::Ledger.compute!(@user)
    # Spread over months, not days: a window shorter than the history is what makes the range
    # control a choice at all, and 30D on a three-day history draws the same picture as ALL.
    (1..3).each do |n|
      # An account with no cash in it, so both readings of a day are the same figures — this file
      # is about the plumbing, and `tracker_show_cash_test.rb` about which pair the switch picks.
      PortfolioSnapshot.create!(user: @user, date: Date.current - (400 - (n * 100)), value_usd: 30_000 + (n * 1_000),
                                invested_usd: 30_000, held_value_usd: 30_000 + (n * 1_000),
                                held_cost_usd: 30_000, partial: false)
    end
    sign_in @user
  end

  def chart_node
    css_select('[data-controller="bot--chart"]').first
  end

  test 'the chart is the bot chart controller fed labels, series and pnl, with both segmented controls wired' do
    get tracker_path
    assert_response :success

    node = chart_node
    assert node, 'the bot chart controller drives the tracker chart'
    assert_equal 3, JSON.parse(node['data-bot--chart-labels-value']).size
    assert_equal [[31_000.0, 32_000.0, 33_000.0], [30_000.0, 30_000.0, 30_000.0]], JSON.parse(node['data-bot--chart-series-value'])
    assert_equal [1_000.0, 2_000.0, 3_000.0], JSON.parse(node['data-bot--chart-pnl-value'])
    assert_equal '0', node['data-bot--chart-bot-value'], 'not a bot — a stable id for the asset pin'
    assert_equal 'USD', node['data-bot--chart-quote-value']
    assert_equal 'false', node['data-bot--chart-pnl-only-value']
    assert_select '.widget--chart__plot canvas[data-bot--chart-target="analyzerChart"]'
    # P/L first and P/L on: the tracker chart opens on the same mode a bot's does, under the same
    # remembered key — one preference, not one per chart.
    assert_select '.widget--chart__modes[data-action*="bot--chart#mode"] > .segmented[data-segmented-key="chart-mode"]' do
      assert_select '.segmented__menu > *:first-child.is-on[data-value="pnl"]', 1
      assert_select '.segmented__option[data-value="value"]:not(.is-on)', 1
    end
    # Both controls on the head line, opposite the readout: the mode redraws the curve, the range
    # narrows the window, and neither belongs below the plot.
    assert_select '.widget--chart__head > .widget--chart__modes', 1
    assert_select '.widget--chart__ranges', 0
    assert_select '.widget--chart__modes .filters[data-action*="bot--chart#range"] > .segmented .segmented__option[data-value="30"]'
    assert_select '.widget--chart__modes .filters[data-action*="bot--chart#range"] > ' \
                  '.segmented[data-segmented-key="chart-range"] .segmented__option.is-on[data-value="all"]'
    assert_select '.widget__placeholder', false
  end

  test 'with balances hidden the chart is pnl-only and the series are normalized — no money in the page' do
    @user.update!(hide_balances: true)
    get tracker_path

    node = chart_node
    assert_equal 'true', node['data-bot--chart-pnl-only-value']
    value, invested = JSON.parse(node['data-bot--chart-series-value'])
    assert_equal [100.0, 100.0, 100.0], invested, 'invested is the 100 baseline'
    assert_in_delta 103.33, value[0], 0.01
    assert_in_delta 110.0, value[2], 0.01
    pnl = JSON.parse(node['data-bot--chart-pnl-value'])
    assert_in_delta 10.0, pnl[2], 0.01
    assert_no_match(/30000|31000|33000|30,000/, response.body)
    assert_select '.widget--chart__modes .segmented__option', false
    assert_select '[data-bot--chart-target="pnl"]', false
    assert_select '.widget--chart__pnl[data-bot--chart-target="percent"]'
  end

  test 'the display currency converts the series' do
    Utilities::Currency.stubs(:exchange_rate).with(from: 'USD', to: 'PLN').returns(Result::Success.new(4.0))
    @user.update!(display_currency: 'PLN')
    get tracker_path

    series = JSON.parse(chart_node['data-bot--chart-series-value'])
    assert_equal [124_000.0, 128_000.0, 132_000.0], series[0]
    assert_equal 'PLN', chart_node['data-bot--chart-quote-value']
  end

  test 'missing coverage back to the first transaction enqueues the backfill once; the plot spins meanwhile' do
    PortfolioSnapshot.for_user(@user).delete_all
    PortfolioSnapshot::BackfillJob.expects(:perform_later).with(@user.id).once
    get tracker_path
    assert_select '.widget--chart__plot .loader', 1, 'a history on its way is a spinner, not an empty axis'
  end

  test 'a history that already reaches back to the first transaction is not backfilled again' do
    # setup: snapshots from 3 days ago, the deposit 3 days ago — covered.
    PortfolioSnapshot::BackfillJob.expects(:perform_later).never
    get tracker_path
  end

  test 'a user whose history starts today is not backfilled on every visit' do
    PortfolioSnapshot.for_user(@user).delete_all
    AccountTransaction.for_user(@user).update_all(transacted_at: Time.current)
    PortfolioSnapshot::BackfillJob.expects(:perform_later).never
    get tracker_path
    assert_response :success
  end

  test 'with balances hidden and both value and invested at zero, the payload is zeros, not NaN' do
    PortfolioSnapshot.for_user(@user).update_all(invested_usd: 0, value_usd: 0, held_cost_usd: 0, held_value_usd: 0)
    @user.update!(hide_balances: true)
    get tracker_path

    value, invested = JSON.parse(chart_node['data-bot--chart-series-value'])
    assert_equal [0.0, 0.0, 0.0], value
    assert_equal [0.0, 0.0, 0.0], invested
  end

  test 'with balances hidden and nothing invested, the payload is still finite and money-free' do
    PortfolioSnapshot.for_user(@user).update_all(invested_usd: 0, held_cost_usd: 0)
    @user.update!(hide_balances: true)
    get tracker_path

    value, invested = JSON.parse(chart_node['data-bot--chart-series-value'])
    assert_equal [0.0, 0.0, 0.0], invested
    assert_in_delta 100.0, value[2], 0.01, 'divided by the last value instead'
    assert_no_match(/31000|33000|31,000/, response.body)
  end
end
