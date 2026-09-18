# frozen_string_literal: true

require 'test_helper'

class CreateSignalBotToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @exchange = create(:binance_exchange)
    create(:ticker, exchange: @exchange, base_asset: create(:asset, :bitcoin), quote_asset: create(:asset, :usd))
    create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)
    @user.set_mcp_tool_enabled('create_signal_bot', true)
    stub_mcp_client(@user)
  end

  teardown { ActionMCP::Current.reset }

  test 'creates the bot and says how to trade through it' do
    text = CreateSignalBotTool.new(exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD')
                              .execute.contents.first.text

    bot = @user.bots.sole
    assert_equal 'Bots::Signal', bot.type
    assert_match(/created and started/, text)
    assert_match(/bot_id #{bot.id}/, text)
  end

  test 'is off until switched on' do
    @user.set_mcp_tool_enabled('create_signal_bot', false)

    text = CreateSignalBotTool.new(exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD')
                              .execute.contents.first.text

    assert_match(/disabled/, text)
    assert_empty @user.bots
  end
end
