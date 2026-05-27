# frozen_string_literal: true

require 'test_helper'

class BotApi::Bots::ListTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
  end

  # ---- shape ---------------------------------------------------------------

  test 'returns a BotApi::Result' do
    result = BotApi::Bots::List.call(user: @user)
    assert_instance_of BotApi::Result, result
  end

  test 'success result has :success status, no error fields, and Hash data' do
    create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current)

    result = BotApi::Bots::List.call(user: @user)

    assert result.success?
    assert_equal :success, result.status
    assert_nil result.error_code
    assert_nil result.error_message
    assert_kind_of Hash, result.data
  end

  test 'empty bots returns count 0 and empty array (not a special-case error)' do
    result = BotApi::Bots::List.call(user: @user)

    assert result.success?
    assert_equal 0, result.data[:count]
    assert_equal [], result.data[:bots]
  end

  # ---- bot rows ------------------------------------------------------------

  test 'each bot row includes id, label, type, pair, exchange, status, interval, quote_amount' do
    bot = create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current)

    result = BotApi::Bots::List.call(user: @user)
    row = result.data[:bots].first

    assert_equal bot.id, row[:id]
    assert_equal bot.label, row[:label]
    assert_equal 'Bots::DcaSingleAsset', row[:type]
    assert_equal 'BTC/USD', row[:pair]
    assert_equal bot.exchange.name, row[:exchange]
    assert_equal 'scheduled', row[:status]
    assert_equal 'day', row[:interval]
    assert_equal 100.0, row[:quote_amount]
    assert_equal 'USD', row[:quote_asset]
  end

  test 'returns count matching the number of bot rows' do
    btc = create(:asset, :bitcoin)
    usd = create(:asset, :usd)
    exchange = create(:binance_exchange)
    3.times do
      create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current,
                                base_asset: btc, quote_asset: usd, exchange: exchange)
    end

    result = BotApi::Bots::List.call(user: @user)

    assert_equal 3, result.data[:count]
    assert_equal 3, result.data[:bots].size
  end

  test 'dual-asset bot pair is base0+base1/quote' do
    create(:dca_dual_asset, user: @user, status: :scheduled, started_at: Time.current)

    result = BotApi::Bots::List.call(user: @user)
    row = result.data[:bots].first

    assert_equal 'BTC+ETH/USD', row[:pair]
    assert_equal 'Bots::DcaDualAsset', row[:type]
  end

  # ---- filtering -----------------------------------------------------------

  test 'filters by status when status: is provided' do
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    exchange = create(:binance_exchange)
    create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current,
                              base_asset: btc, quote_asset: usd, exchange: exchange)
    create(:dca_single_asset, :stopped, user: @user, base_asset: eth, quote_asset: usd, exchange: exchange)

    result = BotApi::Bots::List.call(user: @user, status: 'scheduled')

    assert_equal 1, result.data[:count]
    assert_equal(['scheduled'], result.data[:bots].map { |b| b[:status] })
  end

  test 'blank status: behaves like no filter' do
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    exchange = create(:binance_exchange)
    create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current,
                              base_asset: btc, quote_asset: usd, exchange: exchange)
    create(:dca_single_asset, :stopped, user: @user, base_asset: eth, quote_asset: usd, exchange: exchange)

    result = BotApi::Bots::List.call(user: @user, status: '')

    assert_equal 2, result.data[:count]
  end

  test 'excludes soft-deleted bots' do
    create(:dca_single_asset, :deleted, user: @user)

    result = BotApi::Bots::List.call(user: @user)

    assert_equal 0, result.data[:count]
  end

  test 'is scoped to the given user' do
    other = create(:user)
    create(:dca_single_asset, user: other, status: :scheduled, started_at: Time.current)

    result = BotApi::Bots::List.call(user: @user)

    assert_equal 0, result.data[:count]
  end

  test 'unknown status filter yields an empty result, not an error' do
    create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current)

    result = BotApi::Bots::List.call(user: @user, status: 'no_such_status')

    assert result.success?
    assert_equal 0, result.data[:count]
  end
end
