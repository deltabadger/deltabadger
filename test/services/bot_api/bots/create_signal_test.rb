# frozen_string_literal: true

require 'test_helper'

# A signal bot made from the API has no webhook rule: it is driven by market_buy / market_sell
# naming it. It is created running, because a stopped one refuses every order.
class BotApi::Bots::CreateSignalTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @exchange = create(:binance_exchange)
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd)
    create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)
  end

  def call(**opts)
    BotApi::Bots::CreateSignal.call(user: @user, exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD', **opts)
  end

  test 'creates a running signal bot with no rules' do
    result = call(label: 'Breakout')

    assert result.success?, result.error_message
    assert_equal :created, result.status
    bot = @user.bots.sole
    assert_equal 'Bots::Signal', bot.type
    assert_predicate bot, :scheduled?
    assert_empty bot.bot_signals
    assert_equal @btc.id, bot.base_asset_id
    assert_equal @usd.id, bot.quote_asset_id
    assert_equal({ id: bot.id, label: 'Breakout', type: 'Bots::Signal', status: 'scheduled', exchange: 'Binance',
                   pair: 'BTC/USD', started_at: bot.started_at.iso8601 }, result.data)
  end

  test 'names what is missing' do
    result = BotApi::Bots::CreateSignal.call(user: @user, exchange_name: 'Binance')

    assert_equal 'missing_required_parameter', result.error_code
    assert_includes result.error_message, 'base_asset'
    assert_includes result.error_message, 'quote_asset'
  end

  test 'refuses an unknown exchange, a missing key and an unknown pair' do
    assert_equal 'exchange_not_found', call(exchange_name: 'Nowhere').error_code
    assert_equal 'pair_not_found', call(base_asset: 'ETH').error_code

    @user.api_keys.destroy_all
    assert_equal 'api_key_missing', call.error_code
    assert_empty @user.bots
  end
end
