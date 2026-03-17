# frozen_string_literal: true

require 'test_helper'

class CreateBotToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @exchange = create(:binance_exchange)
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum)
    @usd = create(:asset, :usd)
    @ticker_btc = create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd)
    @ticker_eth = create(:ticker, exchange: @exchange, base_asset: @eth, quote_asset: @usd)
    @api_key = create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)
    ActionMCP::Current.user = @user
    AppConfig.set_mcp_tool_enabled('create_bot', true)
  end

  teardown do
    ActionMCP::Current.reset
    AppConfig.delete(AppConfig::MCP_TOOL_PERMISSIONS)
  end

  # --- Single Asset ---

  test 'creates a single asset DCA bot' do
    Bot::ActionJob.stubs(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    response = CreateBotTool.new(
      exchange_name: 'Binance',
      base_asset: 'BTC',
      quote_asset: 'USD',
      quote_amount: 50.0,
      interval: 'day'
    ).execute

    assert_match(/created and started/i, response.contents.first.text)
    bot = @user.bots.last
    assert bot.dca_single_asset?
    assert bot.working?
    assert_equal 50.0, bot.quote_amount
    assert_equal 'day', bot.settings['interval']
    assert_equal @btc.id, bot.base_asset_id
    assert_equal @usd.id, bot.quote_asset_id
    assert_equal @exchange.id, bot.exchange_id
  end

  test 'creates a single asset bot with custom label' do
    Bot::ActionJob.stubs(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    response = CreateBotTool.new(
      exchange_name: 'Binance',
      base_asset: 'BTC',
      quote_asset: 'USD',
      quote_amount: 100.0,
      interval: 'week',
      label: 'My BTC Bot'
    ).execute

    assert_match(/created and started/i, response.contents.first.text)
    assert_equal 'My BTC Bot', @user.bots.last.label
  end

  # --- Dual Asset ---

  test 'creates a dual asset DCA bot' do
    Bot::ActionJob.stubs(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    response = CreateBotTool.new(
      exchange_name: 'Binance',
      base_asset: 'BTC',
      second_base_asset: 'ETH',
      quote_asset: 'USD',
      quote_amount: 100.0,
      interval: 'week',
      allocation: 60
    ).execute

    assert_match(/created and started/i, response.contents.first.text)
    bot = @user.bots.last
    assert bot.dca_dual_asset?
    assert bot.working?
    assert_equal 100.0, bot.quote_amount
    assert_equal 0.6, bot.allocation0
    assert_equal @btc.id, bot.base0_asset_id
    assert_equal @eth.id, bot.base1_asset_id
  end

  test 'returns error for invalid allocation' do
    response = CreateBotTool.new(
      exchange_name: 'Binance',
      base_asset: 'BTC',
      second_base_asset: 'ETH',
      quote_asset: 'USD',
      quote_amount: 100.0,
      interval: 'day',
      allocation: 150
    ).execute

    assert_match(/Invalid allocation/, response.contents.first.text)
  end

  test 'dual asset bot defaults allocation to 50' do
    Bot::ActionJob.stubs(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    CreateBotTool.new(
      exchange_name: 'Binance',
      base_asset: 'BTC',
      second_base_asset: 'ETH',
      quote_asset: 'USD',
      quote_amount: 100.0,
      interval: 'day'
    ).execute

    assert_equal 0.5, @user.bots.last.allocation0
  end

  # --- Validation errors ---

  test 'returns error when exchange not found' do
    response = CreateBotTool.new(
      exchange_name: 'NonExistent',
      base_asset: 'BTC',
      quote_asset: 'USD',
      quote_amount: 50.0,
      interval: 'day'
    ).execute

    assert_match(/not found/, response.contents.first.text)
  end

  test 'returns error when no valid API key' do
    @api_key.destroy

    response = CreateBotTool.new(
      exchange_name: 'Binance',
      base_asset: 'BTC',
      quote_asset: 'USD',
      quote_amount: 50.0,
      interval: 'day'
    ).execute

    assert_match(/No valid API key/, response.contents.first.text)
  end

  test 'returns error when base asset not found' do
    response = CreateBotTool.new(
      exchange_name: 'Binance',
      base_asset: 'DOGE',
      quote_asset: 'USD',
      quote_amount: 50.0,
      interval: 'day'
    ).execute

    assert_match(/not found on Binance/, response.contents.first.text)
  end

  test 'returns error when trading pair not found' do
    create(:asset, :usdt)

    response = CreateBotTool.new(
      exchange_name: 'Binance',
      base_asset: 'BTC',
      quote_asset: 'USDT',
      quote_amount: 50.0,
      interval: 'day'
    ).execute

    assert_match(/not found on Binance/, response.contents.first.text)
  end

  test 'returns error when second base asset ticker not found' do
    create(:asset, external_id: 'dogecoin', symbol: 'DOGE', name: 'Dogecoin')

    response = CreateBotTool.new(
      exchange_name: 'Binance',
      base_asset: 'BTC',
      second_base_asset: 'DOGE',
      quote_asset: 'USD',
      quote_amount: 50.0,
      interval: 'day'
    ).execute

    assert_match(/not found on Binance/, response.contents.first.text)
  end

  test 'returns error for invalid interval' do
    response = CreateBotTool.new(
      exchange_name: 'Binance',
      base_asset: 'BTC',
      quote_asset: 'USD',
      quote_amount: 50.0,
      interval: 'minute'
    ).execute

    assert_match(/Invalid interval/, response.contents.first.text)
  end

  test 'returns error when tool is disabled' do
    AppConfig.set_mcp_tool_enabled('create_bot', false)

    response = CreateBotTool.new(
      exchange_name: 'Binance',
      base_asset: 'BTC',
      quote_asset: 'USD',
      quote_amount: 50.0,
      interval: 'day'
    ).execute

    assert_match(/disabled/, response.contents.first.text)
  end

  test 'returns error when quote_amount is zero' do
    response = CreateBotTool.new(
      exchange_name: 'Binance',
      base_asset: 'BTC',
      quote_asset: 'USD',
      quote_amount: 0,
      interval: 'day'
    ).execute

    assert_match(/greater than 0/, response.contents.first.text)
  end
end
