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

  test 'unlinked deposit fee in the deposited asset shrinks only the lot quantity' do
    transactions = [
      { entry_type: :deposit, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, fee_currency: 'BTC', fee_amount: '0.001'.to_d,
        transacted_at: Time.utc(2024, 1, 1), tx_id: 'deposit-fee' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: '0.999'.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 2, 1), tx_id: 'deposit-sale' }
    ]

    disposal = @fifo.calculate(transactions).first

    assert_equal '0.999'.to_d, disposal[:amount]
    assert_equal 10_000.to_d, disposal[:cost_basis].round(0)
  end

  test 'taxable swap_in fee in the received asset shrinks only the lot quantity' do
    transactions = [
      { entry_type: :swap_in, base_currency: 'ETH', base_amount: 10.to_d,
        fiat_value: 20_000.to_d, fee_currency: 'ETH', fee_amount: '0.1'.to_d,
        transacted_at: Time.utc(2024, 1, 1), tx_id: 'swap-in-fee' },
      { entry_type: :sell, base_currency: 'ETH', base_amount: '9.9'.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2024, 2, 1), tx_id: 'swap-in-sale' }
    ]

    disposal = @fifo.calculate(transactions).first

    assert_equal '9.9'.to_d, disposal[:amount]
    assert_equal 20_000.to_d, disposal[:cost_basis].round(0)
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
      { entry_type: :sell, base_currency: 'BNB', base_amount: '9.99'.to_d, fiat_value: 2_000.to_d,
        transacted_at: Time.utc(2024, 4, 1), tx_id: 'cross-4' }
    ]

    disposals = Tax::Methods::Fifo.new.calculate(txs)
    btc_disposal = disposals.find { |disposal| disposal[:asset] == 'BTC' }
    bnb_disposal = disposals.find { |disposal| disposal[:asset] == 'BNB' }

    assert_equal 30_001.to_d, btc_disposal[:cost_basis] # 30 000 purchase + 1 BNB-fee value
    assert_equal '9.99'.to_d, bnb_disposal[:amount]
    assert_equal 999.to_d, bnb_disposal[:cost_basis]    # 9.99 BNB remain at 100 each after the 0.01 fee
  end

  test 'fee paid in quote currency is capitalised without consuming quote inventory' do
    transactions = [
      { entry_type: :buy, base_currency: 'USDT', base_amount: 1_000.to_d,
        fiat_value: 1_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'quote-1' },
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d, quote_currency: 'USDT',
        fiat_value: 30_000.to_d, fee_currency: 'USDT', fee_amount: 25.to_d, fee_fiat_value: 25.to_d,
        transacted_at: Time.utc(2024, 1, 2), tx_id: 'quote-2' },
      { entry_type: :sell, base_currency: 'USDT', base_amount: 1_000.to_d,
        fiat_value: 1_000.to_d, transacted_at: Time.utc(2024, 3, 1), tx_id: 'quote-3' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 40_000.to_d, transacted_at: Time.utc(2024, 4, 1), tx_id: 'quote-4' }
    ]

    disposals = @fifo.calculate(transactions)
    btc_disposal = disposals.find { |disposal| disposal[:asset] == 'BTC' }
    usdt_disposal = disposals.find { |disposal| disposal[:asset] == 'USDT' }

    assert_equal 30_025.to_d, btc_disposal[:cost_basis]
    assert_equal 1_000.to_d, usdt_disposal[:cost_basis]
  end

  test 'split adjustment scales quantity and preserves basis and dates' do
    txs = [
      { entry_type: :buy, base_currency: 'NVDA', base_amount: 10.to_d,
        fiat_value: 1_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 's1' },
      { entry_type: :adjustment, base_currency: 'NVDA', base_amount: 90.to_d, # 10:1 split
        fiat_value: 0.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 's2' },
      { entry_type: :sell, base_currency: 'NVDA', base_amount: 100.to_d,
        fiat_value: 2_000.to_d, transacted_at: Time.utc(2024, 7, 1), tx_id: 's3' }
    ]
    d = Tax::Methods::Fifo.new.calculate(txs).first
    assert_equal 1_000.to_d, d[:cost_basis]
    assert_equal Time.utc(2024, 1, 1), d[:acquisition_date]
  end

  test 'split preserves dates and basis across multiple lots' do
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
        fiat_value: 2_000.to_d, transacted_at: Time.utc(2024, 7, 1), tx_id: 'split-sell-1',
        exchange: 'alpaca' },
      { entry_type: :sell, base_currency: 'NVDA', base_amount: 100.to_d,
        fiat_value: 4_000.to_d, transacted_at: Time.utc(2024, 8, 1), tx_id: 'split-sell-2',
        exchange: 'alpaca' }
    ]

    disposals = Tax::Methods::Fifo.new.calculate(transactions)

    assert_equal 1_000.to_d, disposals.first[:cost_basis]
    assert_equal Time.utc(2024, 1, 1), disposals.first[:acquisition_date]
    assert_equal 3_000.to_d, disposals.second[:cost_basis]
    assert_equal Time.utc(2024, 3, 1), disposals.second[:acquisition_date]
  end

  # Guards the shape of a future rewrite, not today's behaviour: with no adjustment branch at all
  # this ledger also produces 1_000. It fails only if a zero delta ever starts clearing or
  # rescaling the pool — it does not prove the branch exists.
  test 'zero-delta adjustment is a no-op' do
    transactions = [
      { entry_type: :buy, base_currency: 'NVDA', base_amount: 10.to_d,
        fiat_value: 1_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'zero-buy', exchange: 'alpaca' },
      { entry_type: :adjustment, base_currency: 'NVDA', base_amount: 0.to_d,
        fiat_value: 0.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'zero-adjustment', exchange: 'alpaca' },
      { entry_type: :sell, base_currency: 'NVDA', base_amount: 10.to_d,
        fiat_value: 2_000.to_d, transacted_at: Time.utc(2024, 7, 1), tx_id: 'zero-sell', exchange: 'alpaca' }
    ]

    disposal = Tax::Methods::Fifo.new.calculate(transactions).first

    assert_equal 1_000.to_d, disposal[:cost_basis]
    assert_equal Time.utc(2024, 1, 1), disposal[:acquisition_date]
  end

  test 'negative adjustment that zeroes the pool clears its lots' do
    transactions = [
      { entry_type: :buy, base_currency: 'NVDA', base_amount: 10.to_d,
        fiat_value: 1_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'clear-buy', exchange: 'alpaca' },
      { entry_type: :adjustment, base_currency: 'NVDA', base_amount: -10.to_d,
        fiat_value: 0.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'clear-adjustment',
        exchange: 'alpaca' },
      { entry_type: :sell, base_currency: 'NVDA', base_amount: 5.to_d,
        fiat_value: 500.to_d, transacted_at: Time.utc(2024, 7, 1), tx_id: 'clear-sell', exchange: 'alpaca' }
    ]

    disposal = Tax::Methods::Fifo.new.calculate(transactions).first

    assert_equal 0.to_d, disposal[:cost_basis]
    assert_equal false, disposal[:cost_basis_complete]
    assert_equal true, disposal[:data_incomplete]
  end

  test 'positive adjustment without a pool opens an assumed-basis lot' do
    transactions = [
      { entry_type: :adjustment, base_currency: 'NVDA', base_amount: 5.to_d,
        fiat_value: 0.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'assumed-adjustment',
        exchange: 'alpaca' },
      { entry_type: :sell, base_currency: 'NVDA', base_amount: 5.to_d,
        fiat_value: 500.to_d, transacted_at: Time.utc(2024, 7, 1), tx_id: 'assumed-sell', exchange: 'alpaca' }
    ]

    disposal = Tax::Methods::Fifo.new.calculate(transactions).first

    assert_equal 0.to_d, disposal[:cost_basis]
    assert_equal true, disposal[:data_incomplete]
    assert_equal true, disposal[:cost_basis_complete] # the lot exists, its basis is the assumed part
  end

  test 'per-share return of capital floors each lot and reports unabsorbed excess' do
    transactions = [
      { entry_type: :buy, base_currency: 'X', base_amount: 10.to_d,
        fiat_value: 50.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'roc-buy-1', exchange: 'alpaca' },
      { entry_type: :buy, base_currency: 'X', base_amount: 10.to_d,
        fiat_value: 1_000.to_d, transacted_at: Time.utc(2024, 2, 1), tx_id: 'roc-buy-2', exchange: 'alpaca' },
      { entry_type: :return_of_capital, base_currency: 'X', base_amount: 20.to_d,
        quote_currency: 'USD', quote_amount: 160.to_d, fiat_value: 160.to_d,
        raw_data: { 'per_share_amount' => '8' }, transacted_at: Time.utc(2024, 3, 1),
        tx_id: 'roc-1', exchange: 'alpaca' },
      { entry_type: :sell, base_currency: 'X', base_amount: 20.to_d,
        fiat_value: 2_000.to_d, transacted_at: Time.utc(2024, 4, 1), tx_id: 'roc-sell', exchange: 'alpaca' }
    ]
    engine = Tax::Methods::Fifo.new

    disposal = engine.calculate(transactions).first

    assert_equal 30.to_d, engine.excess_roc
    assert_equal 920.to_d, disposal[:cost_basis]
  end

  test 'per-share return of capital uses the entry FX multiplier' do
    transactions = [
      { entry_type: :buy, base_currency: 'X', base_amount: 10.to_d,
        fiat_value: 1_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'fx-buy', exchange: 'alpaca' },
      { entry_type: :return_of_capital, base_currency: 'X', base_amount: 10.to_d,
        quote_currency: 'USD', quote_amount: 160.to_d, fiat_value: 320.to_d,
        raw_data: { 'per_share_amount' => '8' }, transacted_at: Time.utc(2024, 3, 1),
        tx_id: 'fx-roc', exchange: 'alpaca' },
      { entry_type: :sell, base_currency: 'X', base_amount: 10.to_d,
        fiat_value: 2_000.to_d, transacted_at: Time.utc(2024, 4, 1), tx_id: 'fx-sell', exchange: 'alpaca' }
    ]
    engine = Tax::Methods::Fifo.new

    disposal = engine.calculate(transactions).first

    assert_equal 840.to_d, disposal[:cost_basis]
    assert_equal 0.to_d, engine.excess_roc
  end

  test 'return of capital without per-share data reduces basis FIFO-dollar and reports excess' do
    transactions = [
      { entry_type: :buy, base_currency: 'X', base_amount: 10.to_d,
        fiat_value: 100.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'dollar-buy', exchange: 'alpaca' },
      { entry_type: :return_of_capital, base_currency: 'X', base_amount: 10.to_d,
        fiat_value: 150.to_d, raw_data: {}, transacted_at: Time.utc(2024, 3, 1),
        tx_id: 'dollar-roc', exchange: 'alpaca' },
      { entry_type: :sell, base_currency: 'X', base_amount: 10.to_d,
        fiat_value: 1_000.to_d, transacted_at: Time.utc(2024, 4, 1), tx_id: 'dollar-sell', exchange: 'alpaca' }
    ]
    engine = Tax::Methods::Fifo.new

    disposal = engine.calculate(transactions).first

    assert_equal 50.to_d, engine.excess_roc
    assert_equal 0.to_d, disposal[:cost_basis]
  end

  test 'return of capital with nothing left to reduce is all excess' do
    transactions = [
      { entry_type: :buy, base_currency: 'X', base_amount: 10.to_d,
        fiat_value: 1_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'sold-buy', exchange: 'alpaca' },
      { entry_type: :sell, base_currency: 'X', base_amount: 10.to_d,
        fiat_value: 2_000.to_d, transacted_at: Time.utc(2024, 2, 1), tx_id: 'sold-sell', exchange: 'alpaca' },
      { entry_type: :return_of_capital, base_currency: 'X', base_amount: 10.to_d,
        quote_currency: 'USD', quote_amount: 80.to_d, fiat_value: 80.to_d,
        raw_data: { 'per_share_amount' => '8' }, transacted_at: Time.utc(2024, 3, 1),
        tx_id: 'sold-roc', exchange: 'alpaca' }
    ]
    engine = Tax::Methods::Fifo.new

    engine.calculate(transactions)

    assert_equal 80.to_d, engine.excess_roc
  end

  test 'withholding tax and unsupported activity have zero effect' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'noop-buy',
        exchange: 'alpaca' },
      { entry_type: :withholding_tax, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 1_000.to_d, transacted_at: Time.utc(2024, 2, 1), tx_id: 'noop-tax',
        exchange: 'alpaca' },
      { entry_type: :unsupported_activity, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 2_000.to_d, transacted_at: Time.utc(2024, 3, 1), tx_id: 'noop-unsupported',
        exchange: 'alpaca' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 4, 1), tx_id: 'noop-sell',
        exchange: 'alpaca' }
    ]

    disposals = Tax::Methods::Fifo.new.calculate(transactions)

    assert_equal 1, disposals.size
    assert_equal 30_000.to_d, disposals.first[:cost_basis]
  end

  test 'fee paid in kind consumes lots without emitting a disposal' do
    transactions = [
      { entry_type: :buy, base_currency: 'ETH', base_amount: 10.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'fee-kind-buy',
        exchange: 'alpaca' },
      { entry_type: :fee, base_currency: 'ETH', base_amount: 1.to_d,
        fiat_value: 0.to_d, transacted_at: Time.utc(2024, 2, 1), tx_id: 'fee-kind', exchange: 'alpaca' },
      { entry_type: :sell, base_currency: 'ETH', base_amount: 10.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2024, 3, 1), tx_id: 'fee-kind-sell',
        exchange: 'alpaca' }
    ]

    disposals = Tax::Methods::Fifo.new.calculate(transactions)

    assert_equal 1, disposals.size
    # The fee burned 1 ETH at 2_000, so only 9 ETH of basis is left for the sale.
    assert_equal 18_000.to_d, disposals.first[:cost_basis]
  end
  # `apply_acquisition_fee` takes a third-asset fee out of that asset's holdings; a disposal has to
  # do the same, or the fee asset's inventory stays overstated and a later sale of it dequeues lots
  # that should not exist.
  test 'a third-asset fee on a disposal leaves that asset inventory' do
    transactions = [
      { entry_type: :buy, base_currency: 'BNB', base_amount: 10.to_d, fiat_value: 1_000.to_d,
        transacted_at: Time.utc(2024, 1, 1), tx_id: 'bnb-buy', exchange: 'binance' },
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d, fiat_value: 20_000.to_d,
        transacted_at: Time.utc(2024, 2, 1), tx_id: 'btc-buy', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d, fiat_value: 30_000.to_d,
        quote_currency: 'EUR', fee_currency: 'BNB', fee_amount: 2.to_d, fee_fiat_value: 200.to_d,
        transacted_at: Time.utc(2024, 3, 1), tx_id: 'btc-sell', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'BNB', base_amount: 10.to_d, fiat_value: 3_000.to_d,
        quote_currency: 'EUR', transacted_at: Time.utc(2024, 4, 1), tx_id: 'bnb-sell',
        exchange: 'binance' }
    ]

    disposals = Tax::Methods::Fifo.new.calculate(transactions)
    bnb_disposal = disposals.find { |disposal| disposal[:asset] == 'BNB' }

    # The 2 BNB fee left the pool at 100 each, so only 8 of the 10 BNB sold carry basis.
    assert_equal 800.to_d, bnb_disposal[:cost_basis]
    assert_equal 2_200.to_d, bnb_disposal[:gain_loss]
  end
end
