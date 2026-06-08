require 'test_helper'

class ExchangeTest < ActiveSupport::TestCase
  # §8 stock-venue routing: a first-class notion of "which exchanges trade stocks"
  # so Alpaca is no longer the hardcoded sole stock venue.
  test 'stock_venue? is true for the stock brokers and false for crypto exchanges' do
    assert_predicate create(:alpaca_exchange), :stock_venue?
    assert_predicate create(:ibkr_exchange), :stock_venue?
    refute_predicate create(:binance_exchange), :stock_venue?
  end

  test 'stock_venues scope returns only the stock brokers' do
    alpaca = create(:alpaca_exchange)
    ibkr = create(:ibkr_exchange)
    create(:binance_exchange)

    assert_equal [alpaca.id, ibkr.id].sort, Exchange.stock_venues.pluck(:id).sort
  end
end
