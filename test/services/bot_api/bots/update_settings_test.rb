# frozen_string_literal: true

require 'test_helper'

class BotApi::Bots::UpdateSettingsTest < ActiveSupport::TestCase
  setup { @user = create(:user) }

  def basket
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    [create(:dca_multi_asset, :stopped, user: @user, base_assets: [btc, eth]), btc, eth]
  end

  test 'retunes an index bot' do
    bot = create(:dca_index, user: @user, status: :stopped)
    result = BotApi::Bots::UpdateSettings.call(user: @user, bot_id: bot.id, num_coins: 8, allocation_flattening: 0.5)
    assert result.success?, result.error_message
    assert_equal %w[allocation_flattening num_coins], result.data[:updated].sort
    bot.reload
    assert_equal 8, bot.num_coins
    assert_equal 0.5, bot.allocation_flattening

    assert_equal 'invalid_number', BotApi::Bots::UpdateSettings.call(user: @user, bot_id: bot.id, num_coins: 'abc').error_code
    assert_equal 'invalid_number', BotApi::Bots::UpdateSettings.call(user: @user, bot_id: bot.id, quote_amount: '1e3').error_code
  end

  test 'index knobs are refused on other bot types' do
    bot = create(:dca_single_asset, :stopped, user: @user)
    assert_equal 'unsupported_setting', BotApi::Bots::UpdateSettings.call(user: @user, bot_id: bot.id, num_coins: 8).error_code
  end

  test 'reweights a basket by symbol' do
    bot, btc, = basket
    result = BotApi::Bots::UpdateSettings.call(user: @user, bot_id: bot.id, allocations: 'BTC:70,ETH:30')
    assert result.success?, result.error_message
    assert_in_delta 0.7, bot.reload.allocation_for(btc.id), 0.0001
    assert_equal 'manual', bot.weighting
  end

  test 'basket weights are refused on an index bot' do
    bot = create(:dca_index, user: @user, status: :stopped)
    assert_equal 'unsupported_setting',
                 BotApi::Bots::UpdateSettings.call(user: @user, bot_id: bot.id, allocations: 'BTC:100').error_code
  end

  test 'weights must cover exactly the basket and sum to 100' do
    bot, btc, = basket
    call = ->(allocations) { BotApi::Bots::UpdateSettings.call(user: @user, bot_id: bot.id, allocations: allocations) }

    assert_equal 'asset_not_in_basket', call.call('BTC:50,SOL:50').error_code
    assert_equal 'missing_basket_asset', call.call('BTC:100').error_code
    assert_equal 'allocations_unbalanced', call.call('BTC:70,ETH:40').error_code
    assert_equal 'invalid_allocations', call.call('BTC:50,btc:50').error_code
    assert_equal 'invalid_allocations', call.call([%w[BTC 50], %w[ETH 50]]).error_code
    assert_equal 'invalid_allocations', call.call('BTC:abc,ETH:40').error_code
    assert_in_delta 0.5, bot.reload.allocation_for(btc.id), 0.0001, 'a refused update must leave the weights alone'
  end

  test 'a hash body reweights the same way a string does' do
    bot, btc, = basket
    result = BotApi::Bots::UpdateSettings.call(user: @user, bot_id: bot.id, allocations: { 'BTC' => 60, 'ETH' => 40 })
    assert result.success?, result.error_message
    assert_in_delta 0.6, bot.reload.allocation_for(btc.id), 0.0001
  end

  test 'a running bot is still refused' do
    bot = create(:dca_index, user: @user, status: :scheduled)
    assert_equal 'bot_running', BotApi::Bots::UpdateSettings.call(user: @user, bot_id: bot.id, num_coins: 8).error_code
  end

  test 'the existing two knobs still work and an empty update is still refused' do
    bot = create(:dca_single_asset, :stopped, user: @user)
    assert BotApi::Bots::UpdateSettings.call(user: @user, bot_id: bot.id, quote_amount: 250, label: 'Renamed').success?
    bot.reload
    assert_equal 250.0, bot.quote_amount
    assert_equal 'Renamed', bot.label
    assert_equal 'no_updates_provided', BotApi::Bots::UpdateSettings.call(user: @user, bot_id: bot.id).error_code
  end
end
