# frozen_string_literal: true

require 'test_helper'

class LimitBuyDryRunTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @exchange = create(:binance_exchange)
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @ticker = create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd)
    @api_key = create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)
    ActionMCP::Current.user = @user
    AppConfig.set_mcp_tool_enabled('limit_buy', true)
  end

  teardown do
    ActionMCP::Current.reset
    AppConfig.clear_mcp_settings!
    Thread.current[:force_dry_run] = nil
  end

  test 'dry run enabled: response contains DRY RUN prefix' do
    AppConfig.mcp_dry_run = true

    order_data = { order_id: 'dry-order-123', status: 'new' }
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:limit_buy).returns(Result::Success.new(order_data))

    response = LimitBuyTool.new(exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD',
                                amount: 100.0, price: 50_000).execute
    text = response.contents.first.text

    assert_match(/\[DRY RUN\]/, text)
    assert_match(/order placed/i, text)
  end

  test 'dry run disabled: response does not contain DRY RUN prefix' do
    AppConfig.mcp_dry_run = false

    order_data = { order_id: '12345', status: 'new' }
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:limit_buy).returns(Result::Success.new(order_data))

    response = LimitBuyTool.new(exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD',
                                amount: 100.0, price: 50_000).execute
    text = response.contents.first.text

    assert_no_match(/\[DRY RUN\]/, text)
    assert_match(/order placed/i, text)
  end

  test 'thread-local is cleaned up after execution' do
    AppConfig.mcp_dry_run = true

    order_data = { order_id: 'dry-order-123', status: 'new' }
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:limit_buy).returns(Result::Success.new(order_data))

    LimitBuyTool.new(exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD',
                     amount: 100.0, price: 50_000).execute

    assert_nil Thread.current[:force_dry_run]
  end
end
