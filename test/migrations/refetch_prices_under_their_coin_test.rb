require 'test_helper'
require Rails.root.join('db/migrate/20260825200000_refetch_prices_under_their_coin.rb')

# Prices fetched before the symbol's coin was resolved by venue and day were fetched under whichever
# asset row came first, and storage being insert-only they would stand forever.
class RefetchPricesUnderTheirCoinTest < ActiveSupport::TestCase
  test 'every date an alias speaks for is cleared, and nothing else' do
    price('LUNA', Date.new(2022, 5, 1))
    kept_luna = price('LUNA', Date.new(2022, 6, 1))
    price('MATIC', Date.new(2021, 7, 1))
    kept_btc = price('BTC', Date.new(2021, 7, 1))
    Tracker::LedgerJob.stubs(:perform_later)

    ActiveRecord::Migration.suppress_messages { RefetchPricesUnderTheirCoin.new.up }

    assert_equal [kept_btc.id, kept_luna.id].sort, HistoricalPrice.pluck(:id).sort
  end

  def price(symbol, date)
    HistoricalPrice.create!(asset: symbol, currency: 'USD', date: date, price: 1)
  end
end
