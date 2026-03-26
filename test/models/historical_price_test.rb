require 'test_helper'

class HistoricalPriceTest < ActiveSupport::TestCase
  test 'stores and retrieves a price' do
    HistoricalPrice.store(asset: 'BTC', currency: 'EUR', date: Date.new(2024, 1, 15), price: 42_000.to_d)

    result = HistoricalPrice.lookup(asset: 'BTC', currency: 'EUR', date: Date.new(2024, 1, 15))
    assert_equal 42_000.to_d, result
  end

  test 'returns nil for missing price' do
    result = HistoricalPrice.lookup(asset: 'BTC', currency: 'EUR', date: Date.new(2020, 1, 1))
    assert_nil result
  end

  test 'store is idempotent (does not raise on duplicate)' do
    HistoricalPrice.store(asset: 'BTC', currency: 'EUR', date: Date.new(2024, 1, 15), price: 42_000.to_d)
    HistoricalPrice.store(asset: 'BTC', currency: 'EUR', date: Date.new(2024, 1, 15), price: 42_000.to_d)

    assert_equal 1, HistoricalPrice.where(asset: 'BTC', currency: 'EUR', date: Date.new(2024, 1, 15)).count
  end

  test 'bulk_store inserts multiple records' do
    records = [
      { asset: 'BTC', currency: 'EUR', date: Date.new(2024, 1, 1), price: 40_000.to_d },
      { asset: 'BTC', currency: 'EUR', date: Date.new(2024, 1, 2), price: 41_000.to_d },
      { asset: 'BTC', currency: 'EUR', date: Date.new(2024, 1, 3), price: 42_000.to_d }
    ]

    HistoricalPrice.bulk_store(records)

    assert_equal 3, HistoricalPrice.where(asset: 'BTC', currency: 'EUR').count
    assert_equal 41_000.to_d, HistoricalPrice.lookup(asset: 'BTC', currency: 'EUR', date: Date.new(2024, 1, 2))
  end

  test 'bulk_store skips existing records' do
    HistoricalPrice.store(asset: 'BTC', currency: 'EUR', date: Date.new(2024, 1, 1), price: 40_000.to_d)

    records = [
      { asset: 'BTC', currency: 'EUR', date: Date.new(2024, 1, 1), price: 99_999.to_d },
      { asset: 'BTC', currency: 'EUR', date: Date.new(2024, 1, 2), price: 41_000.to_d }
    ]

    HistoricalPrice.bulk_store(records)

    # Original price preserved, not overwritten
    assert_equal 40_000.to_d, HistoricalPrice.lookup(asset: 'BTC', currency: 'EUR', date: Date.new(2024, 1, 1))
    assert_equal 2, HistoricalPrice.where(asset: 'BTC', currency: 'EUR').count
  end

  test 'different currencies stored independently' do
    HistoricalPrice.store(asset: 'BTC', currency: 'EUR', date: Date.new(2024, 1, 1), price: 40_000.to_d)
    HistoricalPrice.store(asset: 'BTC', currency: 'CHF', date: Date.new(2024, 1, 1), price: 38_000.to_d)

    assert_equal 40_000.to_d, HistoricalPrice.lookup(asset: 'BTC', currency: 'EUR', date: Date.new(2024, 1, 1))
    assert_equal 38_000.to_d, HistoricalPrice.lookup(asset: 'BTC', currency: 'CHF', date: Date.new(2024, 1, 1))
  end
end
