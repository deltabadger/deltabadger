# frozen_string_literal: true

require 'test_helper'

class CancelOrderToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @bot = create(:dca_single_asset, user: @user)
    @exchange = @bot.exchange
    ActionMCP::Current.user = @user
    AppConfig.set_mcp_tool_enabled('cancel_order', true)
  end

  teardown do
    ActionMCP::Current.reset
    AppConfig.delete(AppConfig::MCP_TOOL_PERMISSIONS)
  end

  test 'cancels an open order by DB ID' do
    transaction = create(:transaction, bot: @bot, exchange: @exchange, status: :submitted, external_status: :open,
                                       side: :buy, order_type: :limit_order, base: 'BTC', quote: 'USD',
                                       amount: 0.5, price: 50_000, external_id: 'order-123')

    Transaction.any_instance.stubs(:cancel).returns(Result::Success.new(transaction))

    response = CancelOrderTool.new(order_id: transaction.id.to_s).execute
    text = response.contents.first.text

    assert_match(/cancel/i, text)
  end

  test 'cancels an order by exchange order ID' do
    alpaca = create(:alpaca_exchange)
    create(:api_key, user: @user, exchange: alpaca, key_type: :trading, status: :correct)

    Exchanges::Alpaca.any_instance.stubs(:set_client)
    Exchanges::Alpaca.any_instance.stubs(:cancel_order).returns(Result::Success.new('abc-uuid-123'))

    response = CancelOrderTool.new(order_id: 'abc-uuid-123', exchange_name: 'Alpaca').execute
    text = response.contents.first.text

    assert_match(/cancel/i, text)
    assert_match(/abc-uuid-123/, text)
    assert_match(/Alpaca/, text)
  end

  test 'requires exchange_name for non-numeric order IDs' do
    response = CancelOrderTool.new(order_id: 'abc-uuid-123').execute
    text = response.contents.first.text

    assert_match(/Exchange name is required/, text)
  end

  test 'returns error when DB order not found and no exchange specified' do
    response = CancelOrderTool.new(order_id: '999999').execute

    assert_match(/Exchange name is required/, response.contents.first.text)
  end

  test 'returns error when cancel fails on exchange' do
    alpaca = create(:alpaca_exchange)
    create(:api_key, user: @user, exchange: alpaca, key_type: :trading, status: :correct)

    Exchanges::Alpaca.any_instance.stubs(:set_client)
    Exchanges::Alpaca.any_instance.stubs(:cancel_order).returns(Result::Failure.new('Order not found'))

    response = CancelOrderTool.new(order_id: 'abc-uuid-123', exchange_name: 'Alpaca').execute
    text = response.contents.first.text

    assert_match(/failed/i, text)
  end

  test 'returns error when cancel fails on DB order' do
    transaction = create(:transaction, bot: @bot, exchange: @exchange, status: :submitted, external_status: :open,
                                       side: :buy, order_type: :limit_order, base: 'BTC', quote: 'USD',
                                       amount: 0.5, price: 50_000, external_id: 'order-456')

    Transaction.any_instance.stubs(:cancel).returns(Result::Failure.new('Exchange rejected cancellation'))

    response = CancelOrderTool.new(order_id: transaction.id.to_s).execute
    text = response.contents.first.text

    assert_match(/failed/i, text)
    assert_match(/Exchange rejected/, text)
  end

  test 'returns error when tool is disabled' do
    AppConfig.set_mcp_tool_enabled('cancel_order', false)
    response = CancelOrderTool.new(order_id: '1').execute

    assert_match(/disabled/, response.contents.first.text)
  end
end
