# frozen_string_literal: true

require 'test_helper'

# Contract for the new cache-only global-PnL path used by the /bots index.
#
# The index must render without making ANY live exchange or market-data calls.
# `User#global_pnl_snapshot(cache_only: true)` reads only what is already cached and
# returns a { result:, loading: } pair with three distinct states:
#
#   * ready   -> { result: { percent:, profit_usd: }, loading: false }
#   * loading -> { result: nil,                        loading: true  }  (a needed cache is cold)
#   * empty   -> { result: nil,                        loading: false }  (nothing invested)
#
# The empty state (e.g. a user whose only bots have no submitted transactions) must NOT
# spin forever. Completeness only considers bots that have submitted transactions.
class UserGlobalPnlSnapshotTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
  end

  # Guard: in cache-only mode nothing may reach the network.
  def assert_no_live_market_calls
    Exchanges::Binance.any_instance.expects(:get_tickers_prices).never
    MarketData.expects(:get_price).never
    MarketData.expects(:get_exchange_rates).never
  end

  test 'ready: returns total and no loading when every included bot metric is cached (USD, no FX)' do
    bot = create(:dca_single_asset, user: @user)
    create(:transaction, bot: bot)
    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices_from_cache).returns(
      { total_quote_amount_invested: 100.0, total_amount_value_in_quote: 120.0 }
    )

    assert_no_live_market_calls
    snapshot = @user.global_pnl_snapshot(cache_only: true)

    assert_equal false, snapshot[:loading]
    assert_in_delta 0.2, snapshot[:result][:percent], 1e-9
    assert_in_delta 20.0, snapshot[:result][:profit_usd], 1e-9
  end

  test 'loading: a bot with submitted transactions but a cold metrics cache yields loading' do
    bot = create(:dca_single_asset, user: @user)
    create(:transaction, bot: bot)
    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices_from_cache).returns(nil)

    assert_no_live_market_calls
    snapshot = @user.global_pnl_snapshot(cache_only: true)

    assert_equal true, snapshot[:loading]
    assert_nil snapshot[:result]
  end

  test 'empty: a bot with no submitted transactions is skipped, so no perpetual spinner' do
    create(:dca_single_asset, user: @user) # no transactions
    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices_from_cache).returns(nil)

    assert_no_live_market_calls
    snapshot = @user.global_pnl_snapshot(cache_only: true)

    assert_equal false, snapshot[:loading]
    assert_nil snapshot[:result]
  end

  test 'loading: a missing FX rate (non-USD quote) yields loading, never a live FX fetch' do
    eur = create(:asset, :eur)
    bot = create(:dca_single_asset, user: @user, quote_asset: eur)
    create(:transaction, bot: bot, quote: 'EUR')
    # Per-bot metrics ARE cached, but the EUR->USD rate is not (null_store => cold).
    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices_from_cache).returns(
      { total_quote_amount_invested: 100.0, total_amount_value_in_quote: 120.0 }
    )

    assert_no_live_market_calls
    snapshot = @user.global_pnl_snapshot(cache_only: true)

    assert_equal true, snapshot[:loading]
    assert_nil snapshot[:result]
  end

  test 'the existing live global_pnl contract (hash | nil) is unchanged for other callers' do
    bot = create(:dca_single_asset, user: @user)
    create(:transaction, bot: bot)
    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices).returns(
      { total_quote_amount_invested: 100.0, total_amount_value_in_quote: 120.0 }
    )

    result = @user.global_pnl # default (live) path

    assert_in_delta 0.2, result[:percent], 1e-9
    assert_in_delta 20.0, result[:profit_usd], 1e-9
  end
end
