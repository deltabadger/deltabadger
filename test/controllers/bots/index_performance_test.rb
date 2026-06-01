# frozen_string_literal: true

require 'test_helper'

# The /bots index must render without any live exchange/FX roundtrip — performance data
# is filled in asynchronously via broadcasts. This pins the "snappy index" guarantee.
class Bots::IndexPerformanceTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true) # satisfies the onboarding gate (an admin must exist)
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    shared = { user: @user, exchange: @exchange, base_asset: @btc, quote_asset: @usd }

    # Two bots so the index doesn't redirect to the single-bot view.
    @traded = create(:dca_single_asset, **shared)
    create(:transaction, bot: @traded)
    create(:dca_single_asset, **shared)

    sign_in @user
  end

  test 'GET /bots makes no live exchange or market-data calls' do
    Exchanges::Binance.any_instance.expects(:get_tickers_prices).never
    MarketData.expects(:get_price).never
    MarketData.expects(:get_exchange_rates).never

    get bots_path

    assert_response :ok
  end

  test 'POST /broadcasts/global_pnl_update enqueues the async global PnL job' do
    User::BroadcastGlobalPnlUpdateJob.expects(:perform_later).once

    post broadcasts_global_pnl_update_path

    assert_response :ok
  end
end
