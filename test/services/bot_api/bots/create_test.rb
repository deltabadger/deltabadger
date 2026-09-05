# frozen_string_literal: true

require 'test_helper'

# Focused coverage for the scheduled-start (`start_at`) behavior added to bot
# creation. The happy-path / validation-error cases for the rest of the service
# are exercised through the MCP tool and REST controller tests.
class BotApi::Bots::CreateTest < ActiveSupport::TestCase
  setup do
    # Fixed pin so future/past start_at math is deterministic.
    @now = Time.utc(2026, 5, 26, 12, 0, 0)
    travel_to @now
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd)
    create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)
  end

  teardown { travel_back }

  def base_params
    { exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD',
      quote_amount: 100, interval: 'day' }
  end

  # ---------- naming ----------

  test 'a bot created without a label is named like one made in the wizard' do
    Bot::ActionJob.stubs(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    result = BotApi::Bots::Create.call(user: @user, **base_params)

    assert result.success?
    assert_equal 'Bitcoin', @user.bots.last.label
  end

  # ---------- immediate start (unchanged behavior) ----------

  test 'without start_at the bot starts immediately' do
    Bot::ActionJob.stubs(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    result = BotApi::Bots::Create.call(user: @user, **base_params)

    assert result.success?
    bot = @user.bots.last
    assert_equal false, bot.start_time_enabled?
    assert_equal @now, bot.started_at
  end

  # ---------- scheduled start ----------

  test 'a future start_at schedules the first action with wait_until and no immediate job' do
    Bot::ActionJob.expects(:set)
                  .with(wait_until: Time.utc(2026, 6, 1, 9, 0, 0))
                  .returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
    Bot::ActionJob.expects(:perform_later).never

    result = BotApi::Bots::Create.call(user: @user, start_at: '2026-06-01T09:00:00Z', **base_params)

    assert result.success?
    bot = @user.bots.last
    assert_equal 'scheduled', bot.status
    assert_equal Time.utc(2026, 6, 1, 9, 0, 0), bot.started_at
    assert_equal Time.utc(2026, 6, 1, 9, 0, 0),
                 Time.find_zone!('UTC').parse(bot.settings['start_at'])
  end

  test 'the serialized result exposes the scheduled started_at' do
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    result = BotApi::Bots::Create.call(user: @user, start_at: '2026-06-01T09:00:00Z', **base_params)

    assert_equal Time.utc(2026, 6, 1, 9, 0, 0), Time.iso8601(result.data[:started_at])
  end

  test 'a naive start_at is interpreted in the user time zone' do
    @user.update!(time_zone: 'Warsaw') # CEST (UTC+2) in June
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    result = BotApi::Bots::Create.call(user: @user, start_at: '2026-06-01T11:00', **base_params)

    assert result.success?
    # 11:00 Warsaw (CEST, UTC+2) == 09:00 UTC.
    assert_equal Time.utc(2026, 6, 1, 9, 0, 0),
                 Time.find_zone!('UTC').parse(@user.bots.last.settings['start_at'])
  end

  # ---------- invalid start_at: fail before persisting ----------

  test 'a past start_at returns a failure and persists no bot' do
    assert_no_difference -> { Bot.count } do
      result = BotApi::Bots::Create.call(user: @user, start_at: '2026-05-20T09:00:00Z', **base_params)

      assert_not result.success?
      assert_equal 'bot_invalid', result.error_code
    end
  end

  test 'a malformed start_at returns a failure and persists no bot' do
    assert_no_difference -> { Bot.count } do
      result = BotApi::Bots::Create.call(user: @user, start_at: 'not-a-real-date', **base_params)

      assert_not result.success?
    end
  end

  test 'a blank start_at fails rather than silently starting immediately' do
    # A caller that sends the key but with an empty value meant to schedule;
    # never fall through to an immediate (real-money) buy.
    Bot::ActionJob.expects(:perform_later).never

    assert_no_difference -> { Bot.count } do
      result = BotApi::Bots::Create.call(user: @user, start_at: '', **base_params)

      assert_not result.success?
      assert_equal 'bot_invalid', result.error_code
    end
  end

  test 'a non-string start_at fails cleanly without raising and persists no bot' do
    assert_no_difference -> { Bot.count } do
      result = nil
      assert_nothing_raised do
        result = BotApi::Bots::Create.call(user: @user, start_at: 1_234_567_890, **base_params)
      end
      assert_not result.success?
    end
  end
  # ---------- baskets ----------

  def solana = create(:asset, symbol: 'SOL', name: 'Solana', external_id: 'solana', category: 'Cryptocurrency')

  def basket_pairs(*assets)
    assets.each { |asset| create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: @usd) }
    Bot::ActionJob.stubs(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
  end

  test 'a three-asset basket with explicit weights' do
    basket_pairs(create(:asset, :ethereum), solana)

    result = BotApi::Bots::Create.call(user: @user, **base_params.except(:base_asset), assets: 'BTC:50,ETH:30,SOL:20')

    assert result.success?, result.error_message
    bot = @user.bots.find(result.data[:id])
    assert_equal 'Bots::DcaMultiAsset', bot.type
    assert_equal %w[BTC ETH SOL], bot.base_assets.map(&:symbol)
    assert_in_delta 0.3, bot.allocation_for(Asset.find_by(symbol: 'ETH').id), 0.0001
    assert_equal 'BTC+ETH+SOL/USD', result.data[:pair]
  end

  test 'no weights means an equal split' do
    basket_pairs(create(:asset, :ethereum))
    result = BotApi::Bots::Create.call(user: @user, **base_params.except(:base_asset),
                                       assets: [{ 'symbol' => 'BTC' }, { 'symbol' => 'ETH' }])
    assert result.success?, result.error_message
    assert_in_delta 0.5, @user.bots.last.allocation_for(@btc.id), 0.0001
  end

  test 'weights must sum to 100' do
    basket_pairs(create(:asset, :ethereum))
    result = BotApi::Bots::Create.call(user: @user, **base_params.except(:base_asset), assets: 'BTC:70,ETH:40')
    assert_equal 'allocations_unbalanced', result.error_code
  end

  test 'market-cap weighting ignores weights' do
    basket_pairs(create(:asset, :ethereum))
    Asset.update_all(market_cap: 1_000_000)
    result = BotApi::Bots::Create.call(user: @user, **base_params.except(:base_asset),
                                       assets: 'BTC,ETH', weighting: 'market_cap')
    assert result.success?, result.error_message
    assert_equal 'market_cap', @user.bots.last.weighting
  end

  test 'basket size is bounded and symbols are unique' do
    assert_equal 'invalid_basket',
                 BotApi::Bots::Create.call(user: @user, **base_params.except(:base_asset), assets: 'BTC').error_code
    assert_equal 'invalid_basket',
                 BotApi::Bots::Create.call(user: @user, **base_params.except(:base_asset), assets: 'BTC:50,btc:50').error_code
  end

  test 'malformed input is a 422, never a 500 or a silent zero' do
    basket_pairs(create(:asset, :ethereum))
    call = ->(**extra) { BotApi::Bots::Create.call(user: @user, **base_params.except(:base_asset), **extra) }
    assert_equal 'invalid_basket', call.call(assets: [1, 2]).error_code
    assert_equal 'invalid_basket', call.call(assets: [%w[BTC 50], %w[ETH 50]]).error_code
    assert_equal 'invalid_basket', call.call(assets: 'BTC:abc,ETH:40').error_code
    assert_equal 'invalid_basket', call.call(assets: 'BTC:1e2,ETH:40').error_code
    assert_equal 'invalid_allocation',
                 BotApi::Bots::Create.call(user: @user, **base_params, second_base_asset: 'ETH', allocation: 'abc').error_code
    assert_equal 'invalid_number', call.call(assets: 'BTC,ETH', quote_amount: 'abc').error_code
    assert_equal 'invalid_weighting', call.call(assets: 'BTC,ETH', weighting: 'vibes').error_code
    assert_equal 0, @user.bots.count
  end

  test 'market-cap weighting needs a market cap on every member' do
    basket_pairs(create(:asset, :ethereum))
    Asset.update_all(market_cap: nil)
    result = BotApi::Bots::Create.call(user: @user, **base_params.except(:base_asset),
                                       assets: 'BTC,ETH', weighting: 'market_cap')
    assert_equal 'market_cap_unavailable', result.error_code
    assert_equal 0, @user.bots.count
  end

  test 'a basket member the venue does not list is named in the refusal' do
    basket_pairs(create(:asset, :ethereum))
    result = BotApi::Bots::Create.call(user: @user, **base_params.except(:base_asset), assets: 'BTC,DOGE')
    assert_equal 'pair_not_found', result.error_code
    assert_match(%r{DOGE/USD}, result.error_message)
  end

  test 'second_base_asset still builds a two-asset basket' do
    basket_pairs(create(:asset, :ethereum))
    result = BotApi::Bots::Create.call(user: @user, **base_params, second_base_asset: 'ETH', allocation: 70)
    assert result.success?, result.error_message
    assert_in_delta 0.7, @user.bots.last.allocation_for(@btc.id), 0.0001
  end

  test 'a single-asset create still needs its base_asset' do
    result = BotApi::Bots::Create.call(user: @user, **base_params.except(:base_asset))
    assert_equal 'missing_required_parameter', result.error_code
    assert_match(/base_asset/, result.error_message)
  end
end
