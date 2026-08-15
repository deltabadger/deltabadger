require 'test_helper'
require Rails.root.join('db/migrate/20260815150060_delete_zero_historical_prices.rb')

# A stored zero cannot heal itself: `HistoricalPrice.store` is insert-or-ignore and
# `fetch_price_range` counts a poisoned date as already covered, so the row survives every report.
class DeleteZeroHistoricalPricesTest < ActiveSupport::TestCase
  test 'poisoned zero rows go, real prices stay' do
    poisoned = HistoricalPrice.create!(asset: 'BTC', currency: 'EUR', date: Date.new(2024, 3, 1), price: 0)
    real = HistoricalPrice.create!(asset: 'BTC', currency: 'EUR', date: Date.new(2024, 3, 2), price: 50_000)

    DeleteZeroHistoricalPrices.new.tap { |migration| migration.verbose = false }.up

    assert_empty HistoricalPrice.where(id: poisoned.id)
    assert HistoricalPrice.exists?(real.id)
  end
end
