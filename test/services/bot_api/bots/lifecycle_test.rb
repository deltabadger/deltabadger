# frozen_string_literal: true

require 'test_helper'

class BotApi::Bots::LifecycleTest < ActiveSupport::TestCase
  setup { @user = create(:user) }

  test 'delete hides a stopped bot and cancels nothing that is not there' do
    bot = create(:dca_single_asset, :stopped, user: @user)
    result = BotApi::Bots::Delete.call(user: @user, bot_id: bot.id)
    assert result.success?, result.error_message
    assert bot.reload.deleted?
    assert_equal 'bot_not_found', BotApi::Bots::Delete.call(user: @user, bot_id: bot.id).error_code
  end

  test 'delete of a running bot cancels its scheduled tick, which the web path never did' do
    bot = create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current)
    Bots::DcaSingleAsset.any_instance.expects(:cancel_scheduled_action_jobs)
    assert BotApi::Bots::Delete.call(user: @user, bot_id: bot.id).success?
    assert bot.reload.deleted?
  end

  test 'delete works for every bot type' do
    # Hoisted, because two bot factories built with their own defaults collide on Asset#external_id
    # and Exchange#type. The index factory defaults to Kraken/EUR, so it needs nothing here.
    exchange = create(:binance_exchange)
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    bots = [create(:dca_multi_asset, :stopped, user: @user, exchange: exchange, base_assets: [btc, eth], quote_asset: usd),
            create(:dca_index, user: @user, status: :stopped),
            create(:signal_bot, :stopped, user: @user, exchange: exchange, base_asset: btc, quote_asset: usd)]

    bots.each do |bot|
      assert BotApi::Bots::Delete.call(user: @user, bot_id: bot.id).success?, bot.type
      assert bot.reload.deleted?
    end
  end

  test 'archive stops and archives; unarchive returns it stopped' do
    bot = create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current)
    Bots::DcaSingleAsset.any_instance.stubs(:cancel_scheduled_action_jobs)
    assert BotApi::Bots::Archive.call(user: @user, bot_id: bot.id).success?
    assert bot.reload.archived?
    assert_equal 'bot_archived', BotApi::Bots::Archive.call(user: @user, bot_id: bot.id).error_code
    assert BotApi::Bots::Unarchive.call(user: @user, bot_id: bot.id).success?
    assert bot.reload.stopped?
    assert_equal 'bot_not_archived', BotApi::Bots::Unarchive.call(user: @user, bot_id: bot.id).error_code
  end

  # Bot::SmartIntervalable's after_initialize dirties `settings` on load, and Bot::Accountable
  # raises on any save that follows without set_missed_quote_amount. #stop guards this; #delete,
  # #archive's second write and #unarchive do not, so the services do it for them.
  test 'a bot loaded fresh from the database is still deletable and unarchivable' do
    bot = create(:dca_single_asset, :stopped, user: @user)
    assert BotApi::Bots::Archive.call(user: @user, bot_id: bot.id).success?
    assert BotApi::Bots::Unarchive.call(user: @user, bot_id: bot.id).success?
    assert_equal 'stopped', bot.reload.status
    assert BotApi::Bots::Delete.call(user: @user, bot_id: bot.id).success?
  end

  test 'every lifecycle call refuses a bot that is not there' do
    assert_equal 'bot_not_found', BotApi::Bots::Delete.call(user: @user, bot_id: 0).error_code
    assert_equal 'bot_not_found', BotApi::Bots::Archive.call(user: @user, bot_id: 0).error_code
    assert_equal 'bot_not_found', BotApi::Bots::Unarchive.call(user: @user, bot_id: 0).error_code
  end
end
