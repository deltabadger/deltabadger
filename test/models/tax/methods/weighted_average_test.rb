require 'test_helper'

class Tax::Methods::WeightedAverageTest < ActiveSupport::TestCase
  setup do
    @wa = Tax::Methods::WeightedAverage.new
  end

  test 'simple buy then sell with average cost' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'sell-1', exchange: 'binance' }
    ]

    disposals = @wa.calculate(transactions)

    assert_equal 1, disposals.size
    assert_equal 30_000.to_d, disposals.first[:cost_basis]
    assert_equal 20_000.to_d, disposals.first[:gain_loss]
  end

  test 'average cost recalculated after multiple buys' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 40_000.to_d, transacted_at: Time.utc(2024, 3, 1), tx_id: 'buy-2', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'sell-1', exchange: 'binance' }
    ]

    disposals = @wa.calculate(transactions)

    assert_equal 1, disposals.size
    # Average cost: (20000 + 40000) / 2 = 30000
    assert_equal 30_000.to_d, disposals.first[:cost_basis]
    assert_equal 20_000.to_d, disposals.first[:gain_loss]
  end

  test 'partial sell uses average cost' do
    transactions = [
      { entry_type: :buy, base_currency: 'ETH', base_amount: 4.to_d,
        fiat_value: 8_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'kraken' },
      { entry_type: :buy, base_currency: 'ETH', base_amount: 6.to_d,
        fiat_value: 18_000.to_d, transacted_at: Time.utc(2024, 3, 1), tx_id: 'buy-2', exchange: 'kraken' },
      { entry_type: :sell, base_currency: 'ETH', base_amount: 5.to_d,
        fiat_value: 15_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'sell-1', exchange: 'kraken' }
    ]

    disposals = @wa.calculate(transactions)

    # Average: (8000 + 18000) / 10 = 2600/ETH. Sell 5 → cost = 13000
    assert_equal 13_000.to_d, disposals.first[:cost_basis]
    assert_equal 2_000.to_d, disposals.first[:gain_loss]
  end

  test 'an unpriced acquisition contaminates the average-cost pool' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 0.to_d, price_missing: true, transacted_at: Time.utc(2024, 1, 1) },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, price_missing: false, transacted_at: Time.utc(2024, 6, 1) }
    ]

    disposal = @wa.calculate(transactions).first

    assert_equal true, disposal[:data_incomplete]
  end

  test 'withdrawal emits no disposal' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1) },
      { entry_type: :withdrawal, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 2, 1) }
    ]

    assert_empty @wa.calculate(transactions)
  end

  test 'linked withdrawal removes only its fee slice at average cost' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1) },
      { entry_type: :withdrawal, base_currency: 'BTC', base_amount: 1.to_d, linked: true,
        transfer_fee_amount: '0.001'.to_d, fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 2, 1) },
      { entry_type: :deposit, base_currency: 'BTC', base_amount: '0.999'.to_d, linked: true,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 2, 1, 1) },
      { entry_type: :sell, base_currency: 'BTC', base_amount: '0.999'.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2024, 3, 1), tx_id: 'sell-1' }
    ]

    disposals = @wa.calculate(transactions)

    assert_equal 1, disposals.size
    assert_equal 'sell-1', disposals.first[:tx_id]
    assert_equal 9_990.to_d, disposals.first[:cost_basis]
  end

  test 'buy fees update weighted-average cost and base-asset quantity' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 30_000.to_d, fee_currency: 'EUR', fee_fiat_value: 50.to_d,
        transacted_at: Time.utc(2024, 1, 1), tx_id: 'fiat-fee-buy' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 2, 1), tx_id: 'fiat-fee-sell' }
    ]

    disposal = @wa.calculate(transactions).first
    assert_equal 30_050.to_d, disposal[:cost_basis]

    transactions = [
      { entry_type: :buy, base_currency: 'BNB', base_amount: 10.to_d,
        fiat_value: 1_000.to_d, fee_currency: 'BNB', fee_amount: '0.01'.to_d, fee_fiat_value: 1.to_d,
        transacted_at: Time.utc(2024, 1, 1), tx_id: 'base-fee-buy' },
      { entry_type: :sell, base_currency: 'BNB', base_amount: '9.99'.to_d,
        fiat_value: 2_000.to_d, transacted_at: Time.utc(2024, 2, 1), tx_id: 'base-fee-sell' }
    ]

    disposal = @wa.calculate(transactions).first
    assert_equal '9.99'.to_d, disposal[:amount]
    assert_equal 1_000.to_d, disposal[:cost_basis].round(0) # 9.99-unit pool retains the full 1 000 quote cost
  end

  test 'fee paid in another crypto capitalises its value and consumes its pool' do
    transactions = [
      { entry_type: :buy, base_currency: 'BNB', base_amount: 10.to_d,
        fiat_value: 1_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'cross-1' },
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 30_000.to_d, fee_currency: 'BNB', fee_amount: '0.01'.to_d, fee_fiat_value: 1.to_d,
        transacted_at: Time.utc(2024, 1, 2), tx_id: 'cross-2' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 40_000.to_d, transacted_at: Time.utc(2024, 3, 1), tx_id: 'cross-3' },
      { entry_type: :sell, base_currency: 'BNB', base_amount: '9.99'.to_d,
        fiat_value: 2_000.to_d, transacted_at: Time.utc(2024, 4, 1), tx_id: 'cross-4' }
    ]

    disposals = @wa.calculate(transactions)
    btc_disposal = disposals.find { |disposal| disposal[:asset] == 'BTC' }
    bnb_disposal = disposals.find { |disposal| disposal[:asset] == 'BNB' }

    assert_equal 30_001.to_d, btc_disposal[:cost_basis]
    assert_equal 999.to_d, bnb_disposal[:cost_basis]
  end
end
