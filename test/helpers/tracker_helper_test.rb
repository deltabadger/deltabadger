require 'test_helper'

class TrackerHelperTest < ActionView::TestCase
  include ApplicationHelper

  Holding = Struct.new(:asset, :value)

  def setup
    @denomination = Denomination.new('USD', 1.to_d)
  end

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

  # == money is written to the cent, here too ==
  #
  # The bot tables round money to the cent whatever the venue publishes; the tracker kept eight
  # places, so the same USDC balance read 0.22812533 on one page and 0.23 on the other. The UNIT
  # decides: a figure written in a currency is written in its cents, whatever it is a figure OF.
  # A quantity of a coin still keeps eight — two would round most tokens away.
  test 'a quantity of a stablecoin is written to the cent' do
    assert_equal '0.23', tracker_amount(0.22812533.to_d, 'USDC')
  end

  test 'a figure in fiat keeps its cents, trailing zeros and all' do
    assert_equal '0.20', tracker_amount(0.2.to_d, 'EUR')
    assert_equal '0.00', tracker_amount(0.003.to_d, 'EUR')
  end

  test 'a quantity of a coin keeps the decimals it is published with' do
    assert_equal '0.00000008', tracker_amount(0.00000008.to_d, 'BTC')
    assert_equal '0.00000008', tracker_amount(0.00000008.to_d)
  end

  test 'a price written in a currency is written in its cents, coin or not' do
    assert_equal '0.09 <small>USDC</small>', tracker_figure(0.09078826.to_d, 'USDC')
    assert_equal '0.86 <small>EUR</small>', tracker_figure(0.859107.to_d, 'EUR')
  end

  test 'a fee in a coin keeps its decimals' do
    assert_equal '0.00000008 <small>BTC</small>', tracker_figure(0.00000008.to_d, 'BTC')
  end
end
