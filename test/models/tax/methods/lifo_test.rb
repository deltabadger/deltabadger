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

  test 'buy fee increases cost basis' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, fee_currency: 'EUR', fee_fiat_value: 50.to_d,
        transacted_at: Time.utc(2024, 1, 1), tx_id: 'fee-buy' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 2, 1), tx_id: 'fee-sell' }
    ]

    disposal = @lifo.calculate(transactions).first

    assert_equal 10_050.to_d, disposal[:cost_basis]
  end

  test 'split scales lots while LIFO preserves the newest basis and date' do
    transactions = [
      { entry_type: :buy, base_currency: 'NVDA', base_amount: 10.to_d,
        fiat_value: 1_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'split-buy-1',
        exchange: 'alpaca' },
      { entry_type: :buy, base_currency: 'NVDA', base_amount: 10.to_d,
        fiat_value: 3_000.to_d, transacted_at: Time.utc(2024, 3, 1), tx_id: 'split-buy-2',
        exchange: 'alpaca' },
      { entry_type: :adjustment, base_currency: 'NVDA', base_amount: 180.to_d,
        fiat_value: 0.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'split-adjustment',
        exchange: 'alpaca' },
      { entry_type: :sell, base_currency: 'NVDA', base_amount: 100.to_d,
        fiat_value: 4_000.to_d, transacted_at: Time.utc(2024, 7, 1), tx_id: 'split-sell',
        exchange: 'alpaca' }
    ]

    disposal = Tax::Methods::Lifo.new.calculate(transactions).first

    assert_equal 3_000.to_d, disposal[:cost_basis]
    assert_equal Time.utc(2024, 3, 1), disposal[:acquisition_date]
  end
end
