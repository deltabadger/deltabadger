# frozen_string_literal: true

require 'test_helper'

class BotApi::Indices::ListTest < ActiveSupport::TestCase
  setup do
    MarketData.stubs(:configured?).returns(true)
    create(:kraken_exchange)
    create(:index, external_id: 'top-coins', source: Index::SOURCE_INTERNAL, name: 'Top coins',
                   top_coins: %w[bitcoin ethereum], available_exchanges: { 'Exchanges::Kraken' => 2 })
    create(:index, external_id: 'nasdaq-100', source: Index::SOURCE_DELTABADGER, name: 'Nasdaq 100',
                   top_coins: %w[aapl msft], available_exchanges: { 'Exchanges::Alpaca' => 2 })
  end

  test 'lists the indices the picker would show, with exchange names' do
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    data = BotApi::Indices::List.call.data
    assert_equal %w[nasdaq-100 top-coins], data[:indices].map { |row| row[:id] }.sort
    top = data[:indices].find { |row| row[:id] == 'top-coins' }
    assert_equal 2, top[:coins]
    assert_equal ['Kraken'], top[:exchanges]
  end

  test 'stock indices are hidden off the Deltabadger data provider, like the picker' do
    MarketDataSettings.stubs(:deltabadger?).returns(false)
    assert_equal(['top-coins'], BotApi::Indices::List.call.data[:indices].map { |row| row[:id] })
  end

  test 'filters to one exchange' do
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    assert_equal(['top-coins'], BotApi::Indices::List.call(exchange_name: 'Kraken').data[:indices].map { |row| row[:id] })
  end

  test 'an unknown exchange is a 404, not an empty list' do
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    assert_equal 'exchange_not_found', BotApi::Indices::List.call(exchange_name: 'Nowhere').error_code
  end

  test 'requires market data' do
    MarketData.stubs(:configured?).returns(false)
    assert_equal 'market_data_not_configured', BotApi::Indices::List.call.error_code
  end
end
