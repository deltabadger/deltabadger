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
    @user.set_mcp_tool_enabled('create_bot', true)
    stub_mcp_client(@user)
  end

  teardown do
    ActionMCP::Current.reset
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

  # --- Two assets ---
  #
  # `second_base_asset` is unchanged as a parameter; the bot behind it is now the same basket type
  # the wizard builds, so `allocation` becomes the first asset's weight rather than allocation0.

  test 'creates a two-asset DCA bot' do
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
    assert bot.dca_multi_asset?
    assert bot.working?
    assert_equal 100.0, bot.quote_amount
    assert_in_delta 0.6, bot.allocation_for(@btc.id), 0.000001
    assert_in_delta 0.4, bot.allocation_for(@eth.id), 0.000001
    assert_equal [@btc.id, @eth.id].sort, bot.bot_index_assets.in_index.pluck(:asset_id).sort
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

  test 'a two-asset bot defaults to an even split' do
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

    assert_equal [0.5, 0.5], @user.bots.last.allocations.values
  end

  test 'a two-asset bot is labelled with both assets' do
    Bot::ActionJob.stubs(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    result = BotApi::Bots::Create.new(
      user: @user, exchange_name: 'Binance', base_asset: 'BTC', second_base_asset: 'ETH',
      quote_asset: 'USD', quote_amount: 100.0, interval: 'day'
    ).call

    assert_equal 'BTC+ETH/USD', result.data[:pair]
  end

  test 'a single-asset request is unaffected' do
    Bot::ActionJob.stubs(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    result = BotApi::Bots::Create.new(
      user: @user, exchange_name: 'Binance', base_asset: 'BTC',
      quote_asset: 'USD', quote_amount: 100.0, interval: 'day'
    ).call

    assert_equal 'Bots::DcaSingleAsset', Bot.find(result.data[:id]).type
    assert_equal 'BTC/USD', result.data[:pair]
  end

  # --- Scheduled start (start_at) ---

  test 'schedules a bot for a future start_at' do
    travel_to Time.utc(2026, 5, 26, 12, 0, 0) do
      Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
      Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
      Bot::ActionJob.expects(:perform_later).never

      response = CreateBotTool.new(
        exchange_name: 'Binance',
        base_asset: 'BTC',
        quote_asset: 'USD',
        quote_amount: 50.0,
        interval: 'day',
        start_at: '2026-06-01T09:00:00Z'
      ).execute

      assert_match(/scheduled/i, response.contents.first.text)
      bot = @user.bots.last
      assert_equal 'scheduled', bot.status
      assert_equal Time.utc(2026, 6, 1, 9, 0, 0), bot.started_at
    end
  end

  test 'returns an error for a past start_at and creates no bot' do
    travel_to Time.utc(2026, 5, 26, 12, 0, 0) do
      response = CreateBotTool.new(
        exchange_name: 'Binance',
        base_asset: 'BTC',
        quote_asset: 'USD',
        quote_amount: 50.0,
        interval: 'day',
        start_at: '2026-05-20T09:00:00Z'
      ).execute

      assert_match(/Failed to create bot/i, response.contents.first.text)
      assert_not @user.bots.exists?, 'a bot with an invalid start_at must not be persisted'
    end
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
    @user.set_mcp_tool_enabled('create_bot', false)

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
  test 'a basket is created from a comma-separated list of symbols and weights' do
    Bot::ActionJob.stubs(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    response = CreateBotTool.new(exchange_name: 'Binance', quote_asset: 'USD', quote_amount: 100.0,
                                 interval: 'day', assets: 'BTC:50,ETH:50').execute

    assert_match(%r{BTC\+ETH/USD}, response.contents.first.text)
    assert_equal 'Bots::DcaMultiAsset', @user.bots.last.type
  end
end
