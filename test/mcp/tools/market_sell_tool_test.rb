# frozen_string_literal: true

require 'test_helper'

class MarketSellToolTest < ActiveSupport::TestCase
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
  end

  test 'executes a market sell order' do
    order_data = { order_id: '12345', status: 'filled' }
    Exchanges::Binance.any_instance.expects(:set_client).with(api_key: @api_key)
    Exchanges::Binance.any_instance.expects(:market_sell).with(
      ticker: @ticker,
      amount: 0.5,
      amount_type: :base
    ).returns(Result::Success.new(order_data))

    response = MarketSellTool.new(exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD', amount: 0.5).execute

    assert_match(/order placed/i, response.contents.first.text)
  end

  test 'returns error when exchange not found' do
    response = MarketSellTool.new(exchange_name: 'NonExistent', base_asset: 'BTC', quote_asset: 'USD', amount: 0.5).execute

    assert_match(/not found/, response.contents.first.text)
  end

  test 'returns error when no valid API key' do
    @api_key.destroy
    response = MarketSellTool.new(exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD', amount: 0.5).execute

    assert_match(/No valid API key/, response.contents.first.text)
  end

  test 'returns error when tool is disabled' do
    @user.set_mcp_tool_enabled('market_sell', false)
    response = MarketSellTool.new(exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD', amount: 0.5).execute

    assert_match(/disabled/, response.contents.first.text)
  end

  test 'returns error on exchange API failure' do
    Exchanges::Binance.any_instance.expects(:set_client).with(api_key: @api_key)
    Exchanges::Binance.any_instance.expects(:market_sell).with(
      ticker: @ticker,
      amount: 0.5,
      amount_type: :base
    ).returns(Result::Failure.new('Insufficient funds'))

    response = MarketSellTool.new(exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD', amount: 0.5).execute

    assert_match(/Insufficient funds/, response.contents.first.text)
  end
end
