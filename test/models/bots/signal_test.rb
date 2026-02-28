require 'test_helper'

class Bots::SignalTest < ActiveSupport::TestCase
  setup do
    @bot = create(:signal_bot)
  end

  test 'store accessors for base_asset_id and quote_asset_id' do
    assert_equal @bot.settings['base_asset_id'], @bot.base_asset_id
    assert_equal @bot.settings['quote_asset_id'], @bot.quote_asset_id
  end

  test 'does not have quote_amount or interval store accessors' do
    assert_not @bot.respond_to?(:quote_amount)
    assert_not @bot.respond_to?(:interval)
  end

  test 'has_many bot_signals' do
    signal1 = create(:bot_signal, bot: @bot)
    signal2 = create(:bot_signal, bot: @bot)
    assert_includes @bot.bot_signals, signal1
    assert_includes @bot.bot_signals, signal2
  end

  test 'has_many bot_signals with dependent destroy' do
    create(:bot_signal, bot: @bot)
    create(:bot_signal, bot: @bot)
    assert_equal 2, @bot.bot_signals.count

    reflection = Bots::Signal.reflect_on_association(:bot_signals)
    assert_equal :destroy, reflection.options[:dependent]
  end

  test 'api_key_type returns trading' do
    assert_equal :trading, @bot.api_key_type
  end

  test 'parse_params extracts base and quote asset ids' do
    result = @bot.parse_params({ base_asset_id: '1', quote_asset_id: '2' })
    assert_equal({ base_asset_id: 1, quote_asset_id: 2 }, result)
  end

  test 'parse_params ignores blank values' do
    result = @bot.parse_params({ base_asset_id: '', quote_asset_id: nil })
    assert_equal({}, result)
  end

  test 'base_asset returns the correct asset' do
    assert_equal Asset.find(@bot.base_asset_id), @bot.base_asset
  end

  test 'quote_asset returns the correct asset' do
    assert_equal Asset.find(@bot.quote_asset_id), @bot.quote_asset
  end

  test 'ticker returns the exchange ticker' do
    assert_predicate @bot.ticker, :present?
    assert_equal @bot.exchange, @bot.ticker.exchange
  end

  test 'decimals returns ticker decimals' do
    decimals = @bot.decimals
    assert_predicate decimals[:base], :present?
    assert_predicate decimals[:quote], :present?
  end

  test 'signal? returns true' do
    assert_predicate @bot, :signal?
    assert_not @bot.dca_single_asset?
  end

  test 'start sets status to scheduled' do
    create(:bot_signal, bot: @bot)
    assert_predicate @bot, :created?
    result = @bot.start
    assert result
    assert_predicate @bot.reload, :scheduled?
    assert_predicate @bot.started_at, :present?
  end

  test 'start fails without signals' do
    assert_predicate @bot, :created?
    result = @bot.start
    assert_not result
    assert_includes @bot.errors[:base], I18n.t('errors.bots.signal_required')
  end

  test 'stop sets status to stopped' do
    create(:bot_signal, bot: @bot)
    @bot.start
    assert_predicate @bot, :scheduled?

    result = @bot.stop
    assert result
    assert_predicate @bot.reload, :stopped?
    assert_predicate @bot.stopped_at, :present?
  end

  test 'delete sets status to deleted' do
    create(:bot_signal, bot: @bot)
    @bot.start

    result = @bot.delete
    assert result
    assert_predicate @bot.reload, :deleted?
  end

  test 'available_exchanges_for_current_settings returns exchanges with matching tickers' do
    exchanges = @bot.available_exchanges_for_current_settings
    assert_includes exchanges, @bot.exchange
  end

  test 'available_assets_for_current_settings returns base assets' do
    assets = @bot.available_assets_for_current_settings(asset_type: :base_asset)
    assert_kind_of ActiveRecord::Relation, assets
  end

  test 'scope signal filters by type' do
    dca_bot = create(:dca_single_asset, user: @bot.user, exchange: @bot.exchange,
                                        base_asset: @bot.base_asset, quote_asset: @bot.quote_asset,
                                        with_api_key: false)
    assert_includes Bot.signal, @bot
    assert_not_includes Bot.signal, dca_bot
  end

  test 'scope not_signal excludes signal bots' do
    dca_bot = create(:dca_single_asset, user: @bot.user, exchange: @bot.exchange,
                                        base_asset: @bot.base_asset, quote_asset: @bot.quote_asset,
                                        with_api_key: false)
    assert_not_includes Bot.not_signal, @bot
    assert_includes Bot.not_signal, dca_bot
  end
end
