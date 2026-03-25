require 'test_helper'

class Tax::Methods::FifoTest < ActiveSupport::TestCase
  setup do
    @fifo = Tax::Methods::Fifo.new
  end

  test 'simple buy then sell calculates gain' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'sell-1', exchange: 'binance',
        fee_fiat_value: 50.to_d }
    ]

    disposals = @fifo.calculate(transactions)

    assert_equal 1, disposals.size
    d = disposals.first
    assert_equal 'BTC', d[:asset]
    assert_equal 50_000.to_d, d[:proceeds]
    assert_equal 30_000.to_d, d[:cost_basis]
    assert_equal 19_950.to_d, d[:gain_loss] # 50000 - 30000 - 50 fee
  end

  test 'FIFO dequeues oldest lot first' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 40_000.to_d, transacted_at: Time.utc(2024, 3, 1), tx_id: 'buy-2', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'sell-1', exchange: 'binance' }
    ]

    disposals = @fifo.calculate(transactions)

    assert_equal 1, disposals.size
    # Should use first lot at 20k, not second at 40k
    assert_equal 20_000.to_d, disposals.first[:cost_basis]
    assert_equal 30_000.to_d, disposals.first[:gain_loss]
  end

  test 'partial lot consumption' do
    transactions = [
      { entry_type: :buy, base_currency: 'ETH', base_amount: 10.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'kraken' },
      { entry_type: :sell, base_currency: 'ETH', base_amount: 3.to_d,
        fiat_value: 9_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'sell-1', exchange: 'kraken' }
    ]

    disposals = @fifo.calculate(transactions)

    assert_equal 1, disposals.size
    # Cost basis: 3 ETH at 2000/ETH = 6000
    assert_equal 6_000.to_d, disposals.first[:cost_basis]
    assert_equal 3_000.to_d, disposals.first[:gain_loss]
  end

  test 'multiple sales across lots' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2024, 3, 1), tx_id: 'buy-2', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.5.to_d,
        fiat_value: 60_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'sell-1', exchange: 'binance' }
    ]

    disposals = @fifo.calculate(transactions)

    assert_equal 1, disposals.size
    # 1 BTC at 10k + 0.5 BTC at 30k = 10000 + 15000 = 25000
    assert_equal 25_000.to_d, disposals.first[:cost_basis]
    assert_equal 35_000.to_d, disposals.first[:gain_loss]
  end

  test 'loss calculation' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'sell-1', exchange: 'binance' }
    ]

    disposals = @fifo.calculate(transactions)

    assert_equal(-20_000.to_d, disposals.first[:gain_loss])
  end

  test 'staking rewards treated as acquisition at zero cost when no fiat_value' do
    transactions = [
      { entry_type: :staking_reward, base_currency: 'ETH', base_amount: 0.1.to_d,
        fiat_value: 200.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'reward-1', exchange: 'kraken' },
      { entry_type: :sell, base_currency: 'ETH', base_amount: 0.1.to_d,
        fiat_value: 300.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'sell-1', exchange: 'kraken' }
    ]

    disposals = @fifo.calculate(transactions)

    assert_equal 1, disposals.size
    assert_equal 200.to_d, disposals.first[:cost_basis]
    assert_equal 100.to_d, disposals.first[:gain_loss]
  end

  test 'multiple assets tracked independently' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-btc', exchange: 'binance' },
      { entry_type: :buy, base_currency: 'ETH', base_amount: 10.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-eth', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'sell-btc', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'ETH', base_amount: 10.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'sell-eth', exchange: 'binance' }
    ]

    disposals = @fifo.calculate(transactions)

    assert_equal 2, disposals.size
    btc_disposal = disposals.find { |d| d[:asset] == 'BTC' }
    eth_disposal = disposals.find { |d| d[:asset] == 'ETH' }
    assert_equal 20_000.to_d, btc_disposal[:gain_loss]
    assert_equal 10_000.to_d, eth_disposal[:gain_loss]
  end
end
