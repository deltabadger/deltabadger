require 'test_helper'

class Bot::TaxLotsTest < ActiveSupport::TestCase
  test 'sells consume lots first-in first-out' do
    lots = [{ amount: 1.to_d, cost: 100.to_d }, { amount: 1.to_d, cost: 200.to_d }]
    Bot::TaxLots.consume(lots, '1.5'.to_d)

    assert_equal [{ amount: '0.5'.to_d, cost: 100.to_d }], lots
    assert_equal 100.to_d, Bot::TaxLots.basis(lots)
  end

  test 'a partly consumed lot of unknown cost stays unknown, and does not crash' do
    lots = [{ amount: 1.to_d, cost: nil }]
    Bot::TaxLots.consume(lots, '0.5'.to_d)

    assert_equal [{ amount: '0.5'.to_d, cost: nil }], lots
    assert Bot::TaxLots.unknown_cost?(lots)
  end

  test 'cost_of prices the first units without consuming them' do
    lots = [{ amount: 1.to_d, cost: 100.to_d }, { amount: 1.to_d, cost: 200.to_d }]

    assert_equal 200.to_d, Bot::TaxLots.cost_of(lots, '1.5'.to_d)
    assert_equal 300.to_d, Bot::TaxLots.cost_of(lots, 5.to_d), 'units beyond the lots carry no cost'
    assert_equal 2, lots.size
  end

  test 'loss_in? is lot by lot: a sale that nets a gain can still carry a losing lot' do
    lots = [{ amount: 1.to_d, cost: 50.to_d }, { amount: 1.to_d, cost: 150.to_d }]

    assert Bot::TaxLots.loss_in?(lots, 2.to_d, 220.to_d), 'both at 110: +60 on the first, -40 on the second'
    assert_not Bot::TaxLots.loss_in?(lots, 1.to_d, 60.to_d), 'only the 50 lot sells, at 60'
    assert Bot::TaxLots.loss_in?(lots, '1.5'.to_d, 150.to_d), 'at 100 each the half of the 150 lot loses'
    assert_not Bot::TaxLots.loss_in?([], 1.to_d, 10.to_d), 'units the bot never bought carry no loss'
  end

  # 100 / 3 is not exact, and multiplying the rounded per-unit price back by the quantity lands just
  # under the cost — which read as a loss and locked the name out of buying for a whole window.
  test 'a break-even sale is not a loss, whatever the division rounds to' do
    lots = [{ amount: 3.to_d, cost: 100.to_d }]

    assert_not Bot::TaxLots.loss_in?(lots, 3.to_d, 100.to_d), 'the proceeds are exactly the lot cost'
    assert Bot::TaxLots.loss_in?(lots, 3.to_d, '99.99'.to_d), 'a penny under is still a loss'
  end

  test 'a lot of unknown cost makes the verdict unknown, never a gain' do
    lots = [{ amount: 1.to_d, cost: nil }, { amount: 1.to_d, cost: 100.to_d }]

    assert_nil Bot::TaxLots.loss_in?(lots, 1.to_d, 100.to_d), 'the unknown lot alone'
    assert Bot::TaxLots.loss_in?(lots, 2.to_d, 150.to_d), 'a known losing lot still says loss (75 against 100)'
    assert_equal 100.to_d, Bot::TaxLots.basis(lots)
    assert Bot::TaxLots.unknown_cost?(lots)
  end

  test 'a split multiplies the units and leaves the cost alone' do
    lots = [{ amount: 1.to_d, cost: 100.to_d }]
    Bot::TaxLots.split!(lots, 10)
    assert_equal [{ amount: 10.to_d, cost: 100.to_d }], lots
  end

  test 'selling more than was ever bought empties the lots' do
    lots = [{ amount: 1.to_d, cost: 100.to_d }]
    Bot::TaxLots.consume(lots, 3.to_d)
    assert_empty lots
  end
end
