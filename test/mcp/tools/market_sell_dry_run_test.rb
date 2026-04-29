# frozen_string_literal: true

require 'test_helper'

class MarketSellDryRunTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @exchange = create(:binance_exchange)
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @ticker = create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd)
    @api_key = create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)
    ActionMCP::Current.user = @user
    @user.set_mcp_tool_enabled('market_sell', true)
  end

  teardown do
    ActionMCP::Current.reset
    Thread.current[:force_dry_run] = nil
  end

  test 'dry run enabled: response contains DRY RUN prefix' do
    @user.mcp_dry_run = true

    Exchanges::Binance.any_instance.expects(:set_client).with(api_key: @api_key)
    Exchanges::Binance.any_instance.stubs(:get_bid_price).returns(Result::Success.new(50_000.0))

    response = MarketSellTool.new(exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD', amount: 0.5).execute
    text = response.contents.first.text

    assert_match(/\[DRY RUN\]/, text)
    assert_match(/order placed/i, text)
    assert_match(/dry-order-/, text)
  end

  test 'dry run disabled: response does not contain DRY RUN prefix' do
    @user.mcp_dry_run = false

    order_data = { order_id: '12345', status: 'filled' }
    Exchanges::Binance.any_instance.expects(:set_client).with(api_key: @api_key)
    Exchanges::Binance.any_instance.expects(:market_sell).with(
      ticker: @ticker,
      amount: 0.5,
      amount_type: :base
    ).returns(Result::Success.new(order_data))

    response = MarketSellTool.new(exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD', amount: 0.5).execute
    text = response.contents.first.text

    assert_no_match(/\[DRY RUN\]/, text)
    assert_match(/order placed/i, text)
  end

  test 'thread-local is cleaned up after execution' do
    @user.mcp_dry_run = true

    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:get_bid_price).returns(Result::Success.new(50_000.0))

    MarketSellTool.new(exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD', amount: 0.5).execute

    assert_nil Thread.current[:force_dry_run]
  end
end
