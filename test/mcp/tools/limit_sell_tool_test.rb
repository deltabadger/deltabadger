# frozen_string_literal: true

require 'test_helper'

class LimitSellToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @exchange = create(:binance_exchange)
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @ticker = create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd)
    @api_key = create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)
    ActionMCP::Current.user = @user
    @user.set_mcp_tool_enabled('limit_sell', true)
  end

  teardown do
    ActionMCP::Current.reset
  end

  test 'executes a limit sell order' do
    order_data = { order_id: '12345', status: 'open' }
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:limit_sell).returns(Result::Success.new(order_data))

    response = LimitSellTool.new(
      exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD', amount: 0.5, price: 60_000.0
    ).execute

    assert_match(/order placed/i, response.contents.first.text)
  end

  test 'returns error when exchange not found' do
    response = LimitSellTool.new(
      exchange_name: 'NonExistent', base_asset: 'BTC', quote_asset: 'USD', amount: 0.5, price: 60_000.0
    ).execute

    assert_match(/not found/, response.contents.first.text)
  end

  test 'returns error when no valid API key' do
    @api_key.destroy
    response = LimitSellTool.new(
      exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD', amount: 0.5, price: 60_000.0
    ).execute

    assert_match(/No valid API key/, response.contents.first.text)
  end

  test 'returns error when tool is disabled' do
    @user.set_mcp_tool_enabled('limit_sell', false)
    response = LimitSellTool.new(
      exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD', amount: 0.5, price: 60_000.0
    ).execute

    assert_match(/disabled/, response.contents.first.text)
  end
end
