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

  test 'withdrawal emits no disposal and leaves its lot for a later sale' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :withdrawal, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 2, 1), tx_id: 'withdrawal-1', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2024, 3, 1), tx_id: 'sell-1', exchange: 'kraken' }
    ]

    disposals = @fifo.calculate(transactions)

    assert_equal 1, disposals.size
    assert_equal 'sell-1', disposals.first[:tx_id]
    assert_equal 10_000.to_d, disposals.first[:cost_basis]
  end

  test 'buy fee increases cost basis; fee paid in base shrinks the lot' do
    txs = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d, fiat_value: 10_000.to_d,
        fee_fiat_value: 50.to_d, fee_currency: 'EUR', transacted_at: Time.utc(2024, 1, 1), tx_id: 'f1' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d, fiat_value: 20_000.to_d,
        fee_fiat_value: 0.to_d, transacted_at: Time.utc(2024, 2, 1), tx_id: 'f2' }
    ]
    d = Tax::Methods::Fifo.new.calculate(txs).first
    assert_equal 10_050.to_d, d[:cost_basis]

    # Fee paid IN the bought asset: the quote spent (fiat_value) already covers the gross
    # amount, so the fee's value must NOT be added again — only the quantity shrinks.
    txs = [
      { entry_type: :buy, base_currency: 'BNB', base_amount: 10.to_d, fiat_value: 1_000.to_d,
        fee_fiat_value: 1.to_d, fee_currency: 'BNB', fee_amount: '0.01'.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'f3' },
      { entry_type: :sell, base_currency: 'BNB', base_amount: '9.99'.to_d, fiat_value: 2_000.to_d,
        fee_fiat_value: 0.to_d, transacted_at: Time.utc(2024, 2, 1), tx_id: 'f4' }
    ]
    d = Tax::Methods::Fifo.new.calculate(txs).first
    assert_equal '9.99'.to_d, d[:amount]
    assert_equal 1_000.to_d, d[:cost_basis].round(0) # full quote cost survives in the shrunken lot, nothing double-counted
  end

  test 'fee paid in another crypto capitalises its value and consumes its inventory' do
    txs = [
      { entry_type: :buy, base_currency: 'BNB', base_amount: 10.to_d, fiat_value: 1_000.to_d,
        transacted_at: Time.utc(2024, 1, 1), tx_id: 'cross-1' },
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d, fiat_value: 30_000.to_d,
        fee_currency: 'BNB', fee_amount: '0.01'.to_d, fee_fiat_value: 1.to_d,
        transacted_at: Time.utc(2024, 1, 2), tx_id: 'cross-2' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d, fiat_value: 40_000.to_d,
        transacted_at: Time.utc(2024, 3, 1), tx_id: 'cross-3' },
      { entry_type: :sell, base_currency: 'BNB', base_amount: 10.to_d, fiat_value: 2_000.to_d,
        transacted_at: Time.utc(2024, 4, 1), tx_id: 'cross-4' }
    ]

    disposals = Tax::Methods::Fifo.new.calculate(txs)
    btc_disposal = disposals.find { |disposal| disposal[:asset] == 'BTC' }
    bnb_disposal = disposals.find { |disposal| disposal[:asset] == 'BNB' }

    assert_equal 30_001.to_d, btc_disposal[:cost_basis] # 30 000 purchase + 1 BNB-fee value
    assert_equal 999.to_d, bnb_disposal[:cost_basis]    # 9.99 BNB remain at 100 each after the 0.01 fee
  end
end
