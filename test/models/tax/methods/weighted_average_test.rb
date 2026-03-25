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
end
