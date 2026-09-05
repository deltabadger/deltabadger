# frozen_string_literal: true

require 'test_helper'

class BotApi::Bots::CreateIndexTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:kraken_exchange)
    @eur = create(:asset, :eur)
    # The quote picker offers a quote only when MINIMUM_SUPPORTED_COINS (3) members of the index
    # trade against it here, so three members, all on EUR.
    @members = [create(:asset, :bitcoin), create(:asset, :ethereum),
                create(:asset, symbol: 'SOL', name: 'Solana', external_id: 'solana', category: 'Cryptocurrency')]
    @members.each { |asset| create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: @eur) }
    create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)
    create(:index, external_id: 'layer-1', source: Index::SOURCE_COINGECKO, name: 'Layer 1',
                   top_coins: %w[bitcoin ethereum solana], available_exchanges: { 'Exchanges::Kraken' => 3 })
    MarketData.stubs(:configured?).returns(true)
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    MarketData.stubs(:get_top_coins).returns(Result::Success.new(%w[bitcoin ethereum solana]))
    # Starting derives the composition; stub exactly what test/models/bots/dca_index_test.rb stubs.
    Bot::ActionJob.stubs(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
  end

  def params(**overrides)
    { exchange_name: 'Kraken', quote_asset: 'EUR', quote_amount: 50, interval: 'week', index: 'layer-1' }.merge(overrides)
  end

  test 'creates and starts a category index bot' do
    result = BotApi::Bots::CreateIndex.call(user: @user, **params(num_coins: 5))

    assert result.success?, result.error_message
    bot = @user.bots.find(result.data[:id])
    assert_equal 'Bots::DcaIndex', bot.type
    assert_equal 'category', bot.index_type
    assert_equal 'layer-1', bot.index_category_id
    assert_equal 'Layer 1', bot.index_name
    assert_equal 5, bot.num_coins
    assert_equal 0.0, bot.allocation_flattening
    assert bot.working?
  end

  test 'defaults to the Top coins index when no index is given' do
    create(:index, external_id: 'top-coins', source: Index::SOURCE_INTERNAL, name: 'Top coins',
                   top_coins: %w[bitcoin ethereum solana], available_exchanges: { 'Exchanges::Kraken' => 3 })
    result = BotApi::Bots::CreateIndex.call(user: @user, **params(index: nil))
    assert result.success?, result.error_message
    assert_equal 'top', @user.bots.find(result.data[:id]).index_type
  end

  test 'a quote with too few index members is refused the way the picker would not offer it' do
    Ticker.where(base_asset: @members.last).delete_all
    result = BotApi::Bots::CreateIndex.call(user: @user, **params)
    assert_equal 'quote_asset_not_found', result.error_code
    assert_match(/Fewer than 3 members/, result.error_message)
  end

  test 'numbers are strict' do
    assert_equal 'invalid_number', BotApi::Bots::CreateIndex.call(user: @user, **params(quote_amount: 'abc')).error_code
    assert_equal 'invalid_number', BotApi::Bots::CreateIndex.call(user: @user, **params(num_coins: '5.5')).error_code
    assert_equal 'invalid_number', BotApi::Bots::CreateIndex.call(user: @user, **params(allocation_flattening: '1.5')).error_code
    assert_equal 0, @user.bots.count
  end

  test 'unknown index' do
    assert_equal 'index_not_found', BotApi::Bots::CreateIndex.call(user: @user, **params(index: 'nope')).error_code
  end

  test 'an index the exchange cannot serve is refused, not silently created' do
    create(:index, external_id: 'gaming', source: Index::SOURCE_COINGECKO, name: 'Gaming',
                   top_coins: %w[bitcoin], available_exchanges: { 'Exchanges::Binance' => 1 })
    result = BotApi::Bots::CreateIndex.call(user: @user, **params(index: 'gaming'))
    assert_equal 'index_not_found', result.error_code
    assert_match(/not available on Kraken/, result.error_message)
  end

  test 'quote asset must trade on the exchange' do
    assert_equal 'quote_asset_not_found', BotApi::Bots::CreateIndex.call(user: @user, **params(quote_asset: 'JPY')).error_code
  end

  test 'the interval is checked against the same list every creator uses' do
    assert_equal 'invalid_interval', BotApi::Bots::CreateIndex.call(user: @user, **params(interval: 'fortnight')).error_code
  end

  test 'requires a trading key on that venue' do
    @user.api_keys.destroy_all
    assert_equal 'api_key_missing', BotApi::Bots::CreateIndex.call(user: @user, **params).error_code
  end

  test 'requires market data' do
    MarketData.stubs(:configured?).returns(false)
    assert_equal 'market_data_not_configured', BotApi::Bots::CreateIndex.call(user: @user, **params).error_code
  end
end
