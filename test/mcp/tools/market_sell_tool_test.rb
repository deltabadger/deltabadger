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
    @user.set_mcp_tool_enabled('market_sell', true)
    stub_mcp_client(@user)
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

  # MCP casts a `number` property to Float; a small one must not be refused as malformed.
  test 'a small base amount is taken as an amount' do
    bot = create(:signal_bot, :started, user: @user, exchange: @exchange, base_asset: @btc, quote_asset: @usd)
    Ticker.any_instance.stubs(:get_bid_price).returns(Result::Success.new(49_000))

    text = MarketSellTool.new(bot_id: bot.id, amount: 0.00005).execute.contents.first.text

    assert_no_match(/must be a number/, text)
  end

  test 'with a bot_id the sale is recorded on the signal bot' do
    bot = create(:signal_bot, :started, user: @user, exchange: @exchange, base_asset: @btc, quote_asset: @usd)
    Ticker.any_instance.stubs(:get_bid_price).returns(Result::Success.new(49_000))
    Exchanges::Binance.any_instance.expects(:market_sell).returns(Result::Success.new(order_id: 'mcp-2'))

    text = MarketSellTool.new(bot_id: bot.id, amount: 0.5).execute.contents.first.text

    assert_match(/placed by bot #{bot.id}/, text)
    assert_predicate bot.transactions.sole, :sell?
  end
end
