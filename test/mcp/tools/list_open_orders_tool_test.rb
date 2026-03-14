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

  test 'lists open orders' do
    create(:transaction, bot: @bot, exchange: @exchange, status: :submitted, external_status: :open,
                         side: :buy, order_type: :limit_order, base: 'BTC', quote: 'USD',
                         amount: 0.5, price: 50_000, external_id: 'order-123')

    response = ListOpenOrdersTool.new.execute
    text = response.contents.first.text

    assert_match(/Open orders/, text)
    assert_match(/BTC/, text)
    assert_match(/USD/, text)
    assert_match(/order-123/, text)
  end

  test 'returns message when no open orders' do
    response = ListOpenOrdersTool.new.execute

    assert_match(/no open orders/i, response.contents.first.text)
  end

  test 'filters by exchange name' do
    create(:transaction, bot: @bot, exchange: @exchange, status: :submitted, external_status: :open,
                         side: :buy, order_type: :limit_order, base: 'BTC', quote: 'USD',
                         amount: 0.5, price: 50_000, external_id: 'order-123')

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
