require 'test_helper'

class TrackerHelperTest < ActionView::TestCase
  include ApplicationHelper

  Holding = Struct.new(:asset, :value)

  def holding(symbol, category, value)
    Holding.new(Asset.new(symbol: symbol, category: category), value.to_d)
  end

  test 'type shares read by value, largest first, stablecoins apart from crypto' do
    holdings = [holding('BTC', 'Cryptocurrency', 80), holding('AAPL', 'Stock', 10),
                holding('USDT', 'Cryptocurrency', 10)]

    assert_equal [['Crypto', 80], ['Stock', 10], ['Stable', 10]], holdings_type_shares(holdings)
  end

  test 'no shares without value, and none that round to zero' do
    assert_empty holdings_type_shares([holding('BTC', 'Cryptocurrency', 0)])
    assert_equal [['Crypto', 100]],
                 holdings_type_shares([holding('BTC', 'Cryptocurrency', 10_000), holding('AAPL', 'Stock', 1)])
  end
end
