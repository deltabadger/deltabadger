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

  test 'cancels an open order' do
    transaction = create(:transaction, bot: @bot, exchange: @exchange, status: :submitted, external_status: :open,
                                       side: :buy, order_type: :limit_order, base: 'BTC', quote: 'USD',
                                       amount: 0.5, price: 50_000, external_id: 'order-123')

    Transaction.any_instance.stubs(:cancel).returns(Result::Success.new(transaction))

    response = CancelOrderTool.new(order_id: transaction.id).execute
    text = response.contents.first.text

    assert_match(/cancel/i, text)
  end

  test 'returns error when order not found' do
    response = CancelOrderTool.new(order_id: 999_999).execute

    assert_match(/not found/i, response.contents.first.text)
  end

  test 'returns error when cancel fails' do
    transaction = create(:transaction, bot: @bot, exchange: @exchange, status: :submitted, external_status: :open,
                                       side: :buy, order_type: :limit_order, base: 'BTC', quote: 'USD',
                                       amount: 0.5, price: 50_000, external_id: 'order-456')

    Transaction.any_instance.stubs(:cancel).returns(Result::Failure.new('Exchange rejected cancellation'))

    response = CancelOrderTool.new(order_id: transaction.id).execute
    text = response.contents.first.text

    assert_match(/failed/i, text)
    assert_match(/Exchange rejected/, text)
  end

  test 'returns error when tool is disabled' do
    AppConfig.set_mcp_tool_enabled('cancel_order', false)
    response = CancelOrderTool.new(order_id: 1).execute

    assert_match(/disabled/, response.contents.first.text)
  end
end
