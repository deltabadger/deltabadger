# frozen_string_literal: true

require 'test_helper'

class LimitBuyToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @exchange = create(:binance_exchange)
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @ticker = create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd)
    @api_key = create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)
    ActionMCP::Current.user = @user
    @user.set_mcp_tool_enabled('limit_buy', true)
  end

  teardown do
    ActionMCP::Current.reset
  end

  test 'executes a limit buy order' do
    order_data = { order_id: '12345', status: 'open' }
    Exchanges::Binance.any_instance.expects(:set_client).with(api_key: @api_key)
    Exchanges::Binance.any_instance.expects(:limit_buy).with(
      ticker: @ticker,
      amount: 100.0,
      amount_type: :quote,
      price: 50_000.0
    ).returns(Result::Success.new(order_data))

    response = LimitBuyTool.new(
      exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD', amount: 100.0, price: 50_000.0
    ).execute

    assert_match(/order placed/i, response.contents.first.text)
  end

  test 'returns error when exchange not found' do
    response = LimitBuyTool.new(
      exchange_name: 'NonExistent', base_asset: 'BTC', quote_asset: 'USD', amount: 100.0, price: 50_000.0
    ).execute

    assert_match(/not found/, response.contents.first.text)
  end

  test 'returns error when no valid API key' do
    @api_key.destroy
    response = LimitBuyTool.new(
      exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD', amount: 100.0, price: 50_000.0
    ).execute

    assert_match(/No valid API key/, response.contents.first.text)
  end

  test 'returns error when tool is disabled' do
    @user.set_mcp_tool_enabled('limit_buy', false)
    response = LimitBuyTool.new(
      exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD', amount: 100.0, price: 50_000.0
    ).execute

    assert_match(/disabled/, response.contents.first.text)
  end

  test 'returns error on exchange API failure' do
    Exchanges::Binance.any_instance.expects(:set_client).with(api_key: @api_key)
    Exchanges::Binance.any_instance.expects(:limit_buy).with(
      ticker: @ticker,
      amount: 100.0,
      amount_type: :quote,
      price: 50_000.0
    ).returns(Result::Failure.new('Insufficient funds'))

    response = LimitBuyTool.new(
      exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD', amount: 100.0, price: 50_000.0
    ).execute

    assert_match(/Insufficient funds/, response.contents.first.text)
  end
end
