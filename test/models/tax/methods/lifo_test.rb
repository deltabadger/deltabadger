require 'test_helper'

class Tax::Methods::LifoTest < ActiveSupport::TestCase
  setup do
    @lifo = Tax::Methods::Lifo.new
  end

  test 'LIFO dequeues most recent lot first' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 40_000.to_d, transacted_at: Time.utc(2024, 3, 1), tx_id: 'buy-2', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'sell-1', exchange: 'binance' }
    ]

    disposals = @lifo.calculate(transactions)

    assert_equal 1, disposals.size
    # LIFO: uses second lot at 40k (most recent), not first at 20k
    assert_equal 40_000.to_d, disposals.first[:cost_basis]
    assert_equal 10_000.to_d, disposals.first[:gain_loss]
    # Acquisition date should be the most recent lot
    assert_equal Time.utc(2024, 3, 1), disposals.first[:acquisition_date]
  end

  test 'LIFO holding days uses most recent lot date' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 40_000.to_d, transacted_at: Time.utc(2024, 5, 1), tx_id: 'buy-2', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'sell-1', exchange: 'binance' }
    ]

    disposals = @lifo.calculate(transactions)

    # Holding days from May 1 to June 1 = 31 days (LIFO uses most recent)
    assert_equal 31, disposals.first[:holding_days]
  end

  test 'partial lot consumption from end' do
    transactions = [
      { entry_type: :buy, base_currency: 'ETH', base_amount: 10.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'kraken' },
      { entry_type: :buy, base_currency: 'ETH', base_amount: 5.to_d,
        fiat_value: 15_000.to_d, transacted_at: Time.utc(2024, 3, 1), tx_id: 'buy-2', exchange: 'kraken' },
      { entry_type: :sell, base_currency: 'ETH', base_amount: 3.to_d,
        fiat_value: 12_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'sell-1', exchange: 'kraken' }
    ]

    disposals = @lifo.calculate(transactions)

    # LIFO: 3 ETH from second lot at 3000/ETH = 9000 cost basis
    assert_equal 9_000.to_d, disposals.first[:cost_basis]
    assert_equal 3_000.to_d, disposals.first[:gain_loss]
  end
end
