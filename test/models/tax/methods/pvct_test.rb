require 'test_helper'

class Tax::Methods::PvctTest < ActiveSupport::TestCase
  setup do
    @pvct = Tax::Methods::Pvct.new
    @price_service = mock('price_service')
  end

  test 'applies PVCT formula: gain = sale_price - (total_cost * sale_price / portfolio_value)' do
    # Buy 1 BTC for 10000 EUR, portfolio grows to 50000
    # Sell 0.5 BTC for 25000 EUR
    # gain = 25000 - (10000 * 25000 / 50000) = 25000 - 5000 = 20000
    @price_service.stubs(:price_at).with(asset: 'BTC', currency: 'EUR', timestamp: anything).returns(50_000.to_d)
    @price_service.stubs(:convert_fiat).returns(1.to_d)

    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance',
        quote_currency: 'EUR' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 0.5.to_d,
        fiat_value: 25_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'sell-1', exchange: 'binance',
        quote_currency: 'EUR', fee_fiat_value: 0 }
    ]

    disposals = @pvct.calculate(transactions, price_service: @price_service, currency: 'EUR')

    assert_equal 1, disposals.size
    d = disposals.first
    assert_equal 25_000.to_d, d[:proceeds]
    assert_equal 10_000.to_d, d[:total_acquisition_cost]
    # portfolio_value = 1 BTC * 50000 = 50000 (before disposal)
    assert_equal 50_000.to_d, d[:portfolio_value]
    # gain = 25000 - (10000 * 25000 / 50000) = 25000 - 5000 = 20000
    assert_equal 20_000.to_d, d[:gain_loss]
  end

  test 'crypto-to-crypto swaps are not taxable' do
    @price_service.stubs(:price_at).returns(0.to_d)

    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance',
        quote_currency: 'EUR' },
      { entry_type: :swap_out, base_currency: 'BTC', base_amount: 0.5.to_d,
        fiat_value: 25_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'swap-out', exchange: 'binance',
        quote_currency: nil, group_id: 'swap_1' },
      { entry_type: :swap_in, base_currency: 'ETH', base_amount: 10.to_d,
        fiat_value: 25_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'swap-in', exchange: 'binance',
        quote_currency: nil, group_id: 'swap_1' }
    ]

    disposals = @pvct.calculate(transactions, price_service: @price_service, currency: 'EUR')
    assert_empty disposals
  end

  test 'total acquisition cost reduces after each disposal' do
    @price_service.stubs(:price_at).with(asset: 'BTC', currency: 'EUR', timestamp: anything).returns(20_000.to_d)

    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance',
        quote_currency: 'EUR' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 0.5.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'sell-1', exchange: 'binance',
        quote_currency: 'EUR', fee_fiat_value: 0 },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 0.5.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 7, 1), tx_id: 'sell-2', exchange: 'binance',
        quote_currency: 'EUR', fee_fiat_value: 0 }
    ]

    disposals = @pvct.calculate(transactions, price_service: @price_service, currency: 'EUR')

    assert_equal 2, disposals.size
    # First sell: total_cost=10000, portfolio=20000 (1 BTC * 20000)
    # allocated = 10000 * 10000 / 20000 = 5000. gain = 10000 - 5000 = 5000
    assert_equal 5_000.to_d, disposals[0][:gain_loss]
    # After first: total_cost = 10000 - 5000 = 5000
    # Second sell: total_cost=5000, portfolio=10000 (0.5 BTC * 20000)
    # allocated = 5000 * 10000 / 10000 = 5000. gain = 10000 - 5000 = 5000
    assert_equal 5_000.to_d, disposals[1][:gain_loss]
  end
end
