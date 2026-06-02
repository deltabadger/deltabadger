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
end
