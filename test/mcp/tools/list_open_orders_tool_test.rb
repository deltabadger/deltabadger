# frozen_string_literal: true

require 'test_helper'

class ListOpenOrdersToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @bot = create(:dca_single_asset, user: @user)
    @exchange = @bot.exchange
    ActionMCP::Current.user = @user
    AppConfig.set_mcp_tool_enabled('list_open_orders', true)
  end

  teardown do
    ActionMCP::Current.reset
    AppConfig.delete(AppConfig::MCP_TOOL_PERMISSIONS)
  end

  test 'lists open orders from local DB' do
    create(:transaction, bot: @bot, exchange: @exchange, status: :submitted, external_status: :open,
                         side: :buy, order_type: :limit_order, base: 'BTC', quote: 'USD',
                         amount: 0.5, price: 50_000, external_id: 'order-123')

    Exchange.stubs(:where).with(available: true).returns(Exchange.none)

    response = ListOpenOrdersTool.new.execute
    text = response.contents.first.text

    assert_match(/Open orders/, text)
    assert_match(/BTC/, text)
    assert_match(/USD/, text)
    assert_match(/order-123/, text)
  end

  test 'lists open orders from exchange API' do
    alpaca = create(:alpaca_exchange)
    create(:api_key, user: @user, exchange: alpaca, key_type: :trading, status: :correct)
    btc = Asset.find_by(symbol: 'BTC') || create(:asset, :bitcoin)
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    ticker = create(:ticker, exchange: alpaca, base_asset: btc, quote_asset: usd)

    exchange_orders = [
      { order_id: 'abc-123', ticker: ticker, price: 241.81, amount: 0.414,
        side: :buy, order_type: :limit_order, status: :open,
        amount_exec: 0, quote_amount: 100, quote_amount_exec: 0, error_messages: [],
        exchange_response: {} }
    ]

    Exchanges::Alpaca.any_instance.stubs(:set_client)
    Exchanges::Alpaca.any_instance.stubs(:list_open_orders).returns(Result::Success.new(exchange_orders))

    response = ListOpenOrdersTool.new.execute
    text = response.contents.first.text

    assert_match(/Open orders/, text)
    assert_match(/abc-123/, text)
    assert_match(/Alpaca/, text)
  end

  test 'deduplicates orders that exist in both DB and exchange' do
    create(:transaction, bot: @bot, exchange: @exchange, status: :submitted, external_status: :open,
                         side: :buy, order_type: :limit_order, base: 'BTC', quote: 'USD',
                         amount: 0.5, price: 50_000, external_id: 'shared-order-id')

    Exchange.stubs(:where).with(available: true).returns(Exchange.none)

    response = ListOpenOrdersTool.new.execute
    text = response.contents.first.text

    assert_match(/Open orders \(1\)/, text)
  end

  test 'returns message when no open orders' do
    Exchange.stubs(:where).with(available: true).returns(Exchange.none)

    response = ListOpenOrdersTool.new.execute

    assert_match(/no open orders/i, response.contents.first.text)
  end

  test 'filters by exchange name' do
    create(:transaction, bot: @bot, exchange: @exchange, status: :submitted, external_status: :open,
                         side: :buy, order_type: :limit_order, base: 'BTC', quote: 'USD',
                         amount: 0.5, price: 50_000, external_id: 'order-123')

    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:list_open_orders).returns(Result::Success.new([]))

    response = ListOpenOrdersTool.new(exchange_name: @exchange.name).execute
    text = response.contents.first.text

    assert_match(/BTC/, text)
  end

  test 'returns error when tool is disabled' do
    AppConfig.set_mcp_tool_enabled('list_open_orders', false)
    response = ListOpenOrdersTool.new.execute

    assert_match(/disabled/, response.contents.first.text)
  end
end
