require 'test_helper'

class Tax::Methods::FifoCryptoToCryptoTest < ActiveSupport::TestCase
  setup do
    @fifo = Tax::Methods::Fifo.new
  end

  test 'crypto-to-crypto not taxable: swap_out does not create disposal' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :swap_out, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'swap-out',
        exchange: 'binance', group_id: 'swap_1', quote_currency: nil },
      { entry_type: :swap_in, base_currency: 'ETH', base_amount: 20.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'swap-in',
        exchange: 'binance', group_id: 'swap_1' }
    ]

    disposals = @fifo.calculate(transactions, crypto_to_crypto_taxable: false)
    assert_empty disposals
  end

  test 'cost basis chains through swap' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :swap_out, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'swap-out',
        exchange: 'binance', group_id: 'swap_1', quote_currency: nil },
      { entry_type: :swap_in, base_currency: 'ETH', base_amount: 20.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'swap-in',
        exchange: 'binance', group_id: 'swap_1' },
      { entry_type: :sell, base_currency: 'ETH', base_amount: 20.to_d,
        fiat_value: 60_000.to_d, transacted_at: Time.utc(2024, 9, 1), tx_id: 'sell-1',
        exchange: 'binance', quote_currency: 'EUR' }
    ]

    disposals = @fifo.calculate(transactions, crypto_to_crypto_taxable: false)

    assert_equal 1, disposals.size
    # Cost basis should be original BTC purchase (10000), not ETH FMV at swap (50000)
    assert_equal 10_000.to_d, disposals.first[:cost_basis]
    assert_equal 50_000.to_d, disposals.first[:gain_loss]
  end

  test 'multi-hop swap chains cost basis' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      # BTC → ETH
      { entry_type: :swap_out, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 3, 1), tx_id: 'swap1-out',
        exchange: 'binance', group_id: 'swap_1', quote_currency: nil },
      { entry_type: :swap_in, base_currency: 'ETH', base_amount: 10.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 3, 1), tx_id: 'swap1-in',
        exchange: 'binance', group_id: 'swap_1' },
      # ETH → SOL
      { entry_type: :swap_out, base_currency: 'ETH', base_amount: 10.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'swap2-out',
        exchange: 'binance', group_id: 'swap_2', quote_currency: nil },
      { entry_type: :swap_in, base_currency: 'SOL', base_amount: 200.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'swap2-in',
        exchange: 'binance', group_id: 'swap_2' },
      # SOL → EUR
      { entry_type: :sell, base_currency: 'SOL', base_amount: 200.to_d,
        fiat_value: 40_000.to_d, transacted_at: Time.utc(2024, 9, 1), tx_id: 'sell-1',
        exchange: 'binance', quote_currency: 'EUR' }
    ]

    disposals = @fifo.calculate(transactions, crypto_to_crypto_taxable: false)

    assert_equal 1, disposals.size
    # Original cost: 10000 BTC purchase. Chained through BTC→ETH→SOL→EUR
    assert_equal 10_000.to_d, disposals.first[:cost_basis]
    assert_equal 30_000.to_d, disposals.first[:gain_loss]
  end

  test 'swap resets holding period when flag set' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :swap_out, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 8, 1), tx_id: 'swap-out',
        exchange: 'binance', group_id: 'swap_1', quote_currency: nil },
      { entry_type: :swap_in, base_currency: 'ETH', base_amount: 10.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 8, 1), tx_id: 'swap-in',
        exchange: 'binance', group_id: 'swap_1' },
      { entry_type: :sell, base_currency: 'ETH', base_amount: 10.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2025, 3, 1), tx_id: 'sell-1',
        exchange: 'binance', quote_currency: 'EUR' }
    ]

    disposals = @fifo.calculate(transactions,
                                crypto_to_crypto_taxable: false, swap_resets_holding_period: true)

    assert_equal 1, disposals.size
    # Holding period from swap date (Aug 1) not buy date (Jan 1)
    # Aug 1 to Mar 1 = 212 days
    assert_equal 212, disposals.first[:holding_days]
    # Cost basis still from original buy
    assert_equal 10_000.to_d, disposals.first[:cost_basis]
  end

  test 'stablecoin disposal treated as fiat when flag set' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :swap_out, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'swap-out',
        exchange: 'binance', quote_currency: 'USDT', group_id: 'swap_1' },
      { entry_type: :swap_in, base_currency: 'USDT', base_amount: 50_000.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'swap-in',
        exchange: 'binance', group_id: 'swap_1' }
    ]

    # Without stablecoin_as_fiat: USDT is crypto, swap not taxable
    disposals = @fifo.calculate(transactions, crypto_to_crypto_taxable: false)
    assert_empty disposals

    # With stablecoin_as_fiat: USDT treated as fiat exit
    disposals = @fifo.calculate(transactions, crypto_to_crypto_taxable: false, stablecoin_as_fiat: true)
    assert_equal 1, disposals.size
    assert_equal 40_000.to_d, disposals.first[:gain_loss]
  end
end
