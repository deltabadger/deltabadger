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

  test 'an unpriced acquisition contaminates every later disposal portfolio-wide' do
    @price_service.stubs(:price_at).returns(1_000.to_d)

    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 2.to_d,
        fiat_value: 0.to_d, price_missing: true, transacted_at: Time.utc(2024, 1, 1) },
      { entry_type: :buy, base_currency: 'ETH', base_amount: 1.to_d,
        fiat_value: 1_000.to_d, price_missing: false, transacted_at: Time.utc(2024, 1, 2) },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 1_000.to_d, price_missing: false, quote_currency: 'EUR',
        transacted_at: Time.utc(2024, 2, 1) },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 1_000.to_d, price_missing: false, quote_currency: 'EUR',
        transacted_at: Time.utc(2024, 3, 1) },
      { entry_type: :sell, base_currency: 'ETH', base_amount: 1.to_d,
        fiat_value: 1_000.to_d, price_missing: false, quote_currency: 'EUR',
        transacted_at: Time.utc(2024, 4, 1) }
    ]

    disposals = @pvct.calculate(transactions, price_service: @price_service, currency: 'EUR')
    btc_disposals = disposals.select { |disposal| disposal[:asset] == 'BTC' }
    btc_flags = btc_disposals.map { |disposal| disposal[:data_incomplete] }
    eth_disposal = disposals.find { |disposal| disposal[:asset] == 'ETH' }

    assert_equal [true, true], btc_flags
    # The ETH sale is allocated from the same portfolio-wide acquisition pool understated by the unpriced BTC buy.
    assert_equal true, eth_disposal[:data_incomplete]
  end

  test 'a zero-priced portfolio holding marks the disposal incomplete' do
    @price_service.stubs(:price_at)
                  .with(asset: 'BTC', currency: 'EUR', timestamp: anything)
                  .returns(0.to_d)
    @price_service.stubs(:price_at)
                  .with(asset: 'ETH', currency: 'EUR', timestamp: anything)
                  .returns(2_000.to_d)

    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 1_000.to_d, price_missing: false, transacted_at: Time.utc(2024, 1, 1) },
      { entry_type: :buy, base_currency: 'ETH', base_amount: 1.to_d,
        fiat_value: 1_000.to_d, price_missing: false, transacted_at: Time.utc(2024, 1, 2) },
      { entry_type: :sell, base_currency: 'ETH', base_amount: 0.5.to_d,
        fiat_value: 1_000.to_d, price_missing: false, quote_currency: 'EUR',
        transacted_at: Time.utc(2024, 2, 1) }
    ]

    disposals = @pvct.calculate(transactions, price_service: @price_service, currency: 'EUR')

    assert_equal true, disposals.first[:data_incomplete]
  end

  test 'withdrawal emits no disposal and leaves aggregate acquisition cost unchanged' do
    @price_service.stubs(:price_at).returns(20_000.to_d)
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1), quote_currency: 'EUR' },
      { entry_type: :withdrawal, base_currency: 'BTC', base_amount: '0.2'.to_d,
        fiat_value: 5_000.to_d, transacted_at: Time.utc(2024, 2, 1), quote_currency: nil },
      { entry_type: :sell, base_currency: 'BTC', base_amount: '0.8'.to_d,
        fiat_value: 16_000.to_d, transacted_at: Time.utc(2024, 3, 1), quote_currency: 'EUR', tx_id: 'sell-1' }
    ]

    disposals = @pvct.calculate(transactions, price_service: @price_service, currency: 'EUR')

    assert_equal 1, disposals.size
    assert_equal 'sell-1', disposals.first[:tx_id]
    assert_equal 10_000.to_d, disposals.first[:total_acquisition_cost]
    # The withdrawn 0.2 is still held, so the portfolio is a whole BTC: allocated cost is
    # 10 000 x 16 000 / 20 000 = 8 000, not the 10 000 a shrunken denominator would allocate.
    assert_equal 20_000.to_d, disposals.first[:portfolio_value]
    assert_equal 8_000.to_d, disposals.first[:gain_loss]
  end

  # Leaving the cost in the pool while dropping the coins from the portfolio value would inflate
  # every later sale's allocated cost — trading F1's over-taxation for under-taxation.
  test 'an unlinked withdrawal keeps its coins in the portfolio value used to prorate cost' do
    @price_service.stubs(:price_at).returns(30_000.to_d)
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1), quote_currency: 'EUR' },
      { entry_type: :withdrawal, base_currency: 'BTC', base_amount: '0.9'.to_d,
        transacted_at: Time.utc(2024, 2, 1), quote_currency: nil },
      { entry_type: :sell, base_currency: 'BTC', base_amount: '0.1'.to_d,
        fiat_value: 3_000.to_d, transacted_at: Time.utc(2024, 3, 1), quote_currency: 'EUR', tx_id: 'sell-1' }
    ]

    disposal = @pvct.calculate(transactions, price_service: @price_service, currency: 'EUR').first

    assert_equal 30_000.to_d, disposal[:portfolio_value] # the full BTC, not the 0.1 left on the exchange
    assert_equal 2_000.to_d, disposal[:gain_loss]        # 3 000 - (10 000 x 3 000 / 30 000)
  end

  test 'linked deposit adds no aggregate cost while unlinked deposit adds assumed cost' do
    @price_service.stubs(:price_at).returns(20_000.to_d)
    transactions = [
      { entry_type: :deposit, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 20_000.to_d, price_missing: false, linked: true, transacted_at: Time.utc(2024, 1, 1) },
      { entry_type: :deposit, base_currency: 'BTC', base_amount: '0.5'.to_d,
        fiat_value: 5_000.to_d, price_missing: false, linked: false, transacted_at: Time.utc(2024, 1, 2) },
      { entry_type: :sell, base_currency: 'BTC', base_amount: '0.1'.to_d,
        fiat_value: 2_000.to_d, price_missing: false, quote_currency: 'EUR', transacted_at: Time.utc(2024, 2, 1) }
    ]

    disposal = @pvct.calculate(transactions, price_service: @price_service, currency: 'EUR').first

    assert_equal 5_000.to_d, disposal[:total_acquisition_cost]
    assert_equal 30_000.to_d, disposal[:portfolio_value]
    assert_equal true, disposal[:data_incomplete]
  end

  # An unpriceable acquisition understates the cost pool, which is why it contaminates the report.
  # A linked deposit's price is never read at all, so it has nothing to understate — flagging it
  # would tell the user their return is unreliable over a number the engine never touched.
  test 'a linked deposit with no price does not contaminate the report' do
    @price_service.stubs(:price_at).returns(20_000.to_d)
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, price_missing: false, transacted_at: Time.utc(2024, 1, 1), quote_currency: 'EUR' },
      { entry_type: :withdrawal, base_currency: 'BTC', base_amount: 1.to_d, linked: true,
        transfer_fee_amount: 0.to_d, price_missing: true, transacted_at: Time.utc(2024, 2, 1) },
      { entry_type: :deposit, base_currency: 'BTC', base_amount: 1.to_d, linked: true,
        fiat_value: 0.to_d, price_missing: true, transacted_at: Time.utc(2024, 2, 1, 1) },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 30_000.to_d, price_missing: false, quote_currency: 'EUR', transacted_at: Time.utc(2024, 3, 1) }
    ]

    disposal = @pvct.calculate(transactions, price_service: @price_service, currency: 'EUR').first

    assert_equal 10_000.to_d, disposal[:total_acquisition_cost]
    assert_equal false, disposal[:data_incomplete]
  end
end
