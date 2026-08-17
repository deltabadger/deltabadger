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

  test 'applies PVCT formula with an acquisition fee in total cost' do
    # Buy 1 BTC for 10 000 EUR plus a 50 EUR fee, portfolio grows to 50 000.
    # Sell 0.5 BTC for 25 000 EUR.
    # gain = 25 000 - (10 050 * 25 000 / 50 000) = 25 000 - 5 025 = 19 975
    @price_service.stubs(:price_at).with(asset: 'BTC', currency: 'EUR', timestamp: anything).returns(50_000.to_d)
    @price_service.stubs(:convert_fiat).returns(1.to_d)

    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-fee', exchange: 'binance',
        quote_currency: 'EUR', fee_currency: 'EUR', fee_amount: 50.to_d, fee_fiat_value: 50.to_d },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 0.5.to_d,
        fiat_value: 25_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'sell-fee', exchange: 'binance',
        quote_currency: 'EUR', fee_fiat_value: 0 }
    ]

    disposal = @pvct.calculate(transactions, price_service: @price_service, currency: 'EUR').first

    assert_equal 10_050.to_d, disposal[:total_acquisition_cost]
    assert_equal 19_975.to_d, disposal[:gain_loss]
  end

  test 'cross-asset acquisition fee does not duplicate cost already in the PVCT numerator' do
    @price_service.stubs(:price_at)
                  .with(asset: 'BTC', currency: 'EUR', timestamp: anything)
                  .returns(40_000.to_d)
    @price_service.stubs(:price_at)
                  .with(asset: 'BNB', currency: 'EUR', timestamp: anything)
                  .returns(100.to_d)
    @price_service.stubs(:convert_fiat).returns(1.to_d)

    transactions = [
      { entry_type: :buy, base_currency: 'BNB', base_amount: 10.to_d,
        fiat_value: 1_000.to_d, transacted_at: Time.utc(2024, 1, 1), quote_currency: 'EUR' },
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 30_000.to_d, fee_currency: 'BNB', fee_amount: '0.01'.to_d, fee_fiat_value: 1.to_d,
        transacted_at: Time.utc(2024, 1, 2), quote_currency: 'EUR' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 40_000.to_d, fee_fiat_value: 0.to_d,
        transacted_at: Time.utc(2024, 6, 1), quote_currency: 'EUR' }
    ]

    disposal = @pvct.calculate(transactions, price_service: @price_service, currency: 'EUR').first

    # The BNB purchase cost is already in the aggregate: 1 000 + 30 000, not 31 001.
    assert_equal 31_000.to_d, disposal[:total_acquisition_cost]
    # Before the sale: 1 BTC at 40 000 plus 9.99 BNB at 100 = 40 999.
    assert_equal 40_999.to_d, disposal[:portfolio_value]
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

  test 'adjustment increases balance without changing total acquisition cost' do
    @price_service.stubs(:price_at).returns(20.to_d)
    transactions = [
      { entry_type: :buy, base_currency: 'NVDA', base_amount: 10.to_d,
        fiat_value: 1_000.to_d, transacted_at: Time.utc(2024, 1, 1), quote_currency: 'EUR',
        tx_id: 'split-buy', exchange: 'alpaca' },
      { entry_type: :adjustment, base_currency: 'NVDA', base_amount: 90.to_d,
        fiat_value: 0.to_d, transacted_at: Time.utc(2024, 3, 1), quote_currency: nil,
        tx_id: 'split-adjustment', exchange: 'alpaca' },
      { entry_type: :sell, base_currency: 'NVDA', base_amount: 10.to_d,
        fiat_value: 200.to_d, transacted_at: Time.utc(2024, 7, 1), quote_currency: 'EUR',
        tx_id: 'split-sell', exchange: 'alpaca' }
    ]

    disposal = Tax::Methods::Pvct.new.calculate(
      transactions, price_service: @price_service, currency: 'EUR'
    ).first

    assert_equal 1_000.to_d, disposal[:total_acquisition_cost]
    assert_equal 2_000.to_d, disposal[:portfolio_value]
  end

  test 'return of capital reduces total acquisition cost and reports excess' do
    @price_service.stubs(:price_at).returns(10.to_d)
    transactions = [
      { entry_type: :buy, base_currency: 'X', base_amount: 10.to_d,
        fiat_value: 50.to_d, transacted_at: Time.utc(2024, 1, 1), quote_currency: 'EUR',
        tx_id: 'roc-buy', exchange: 'alpaca' },
      { entry_type: :return_of_capital, base_currency: 'X', base_amount: 10.to_d,
        quote_currency: 'USD', quote_amount: 80.to_d, fiat_value: 80.to_d,
        raw_data: { 'per_share_amount' => '8' }, transacted_at: Time.utc(2024, 3, 1),
        tx_id: 'roc', exchange: 'alpaca' },
      { entry_type: :sell, base_currency: 'X', base_amount: 10.to_d,
        fiat_value: 100.to_d, transacted_at: Time.utc(2024, 7, 1), quote_currency: 'EUR',
        tx_id: 'roc-sell', exchange: 'alpaca' }
    ]
    engine = Tax::Methods::Pvct.new

    disposal = engine.calculate(transactions, price_service: @price_service, currency: 'EUR').first

    assert_equal 0.to_d, disposal[:total_acquisition_cost]
    assert_equal 100.to_d, disposal[:portfolio_value]
    assert_equal 30.to_d, engine.excess_roc
  end

  test 'withholding tax and unsupported activity leave balance and cost unchanged' do
    @price_service.stubs(:price_at).returns(20_000.to_d)
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1), quote_currency: 'EUR',
        tx_id: 'noop-buy', exchange: 'alpaca' },
      { entry_type: :withholding_tax, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 1_000.to_d, transacted_at: Time.utc(2024, 2, 1), quote_currency: 'EUR',
        tx_id: 'noop-tax', exchange: 'alpaca' },
      { entry_type: :unsupported_activity, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 2_000.to_d, transacted_at: Time.utc(2024, 3, 1), quote_currency: nil,
        tx_id: 'noop-unsupported', exchange: 'alpaca' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 4, 1), quote_currency: 'EUR',
        tx_id: 'noop-sell', exchange: 'alpaca' }
    ]

    disposals = Tax::Methods::Pvct.new.calculate(
      transactions, price_service: @price_service, currency: 'EUR'
    )

    assert_equal 1, disposals.size
    assert_equal 10_000.to_d, disposals.first[:total_acquisition_cost]
    assert_equal 20_000.to_d, disposals.first[:portfolio_value]
  end
  # Art. 150 VH bis defines the prix de cession net of the frais of that cession, and that net figure
  # is also the numerator of the allocation ratio: (C-f)(1 - A/V), never C - A·C/V - f. PVCT printed
  # the fee in its own column and subtracted it from neither.
  test 'a disposal fee is deducted from the prix de cession, not just from the gain' do
    @price_service.stubs(:price_at).with(asset: 'BTC', currency: 'EUR', timestamp: anything).returns(50_000.to_d)
    @price_service.stubs(:convert_fiat).returns(1.to_d)

    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'cession-buy',
        exchange: 'kraken', quote_currency: 'EUR' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: '0.5'.to_d,
        fiat_value: 25_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'cession-sell',
        exchange: 'kraken', quote_currency: 'EUR', fee_currency: 'EUR', fee_amount: 78.to_d,
        fee_fiat_value: 78.to_d }
    ]

    disposal = @pvct.calculate(transactions, price_service: @price_service, currency: 'EUR').first

    assert_equal 78.to_d, disposal[:fee]
    # net cession 25 000 - 78 = 24 922; allocated cost 10 000 * 24 922 / 50 000 = 4 984.40
    assert_equal '19937.6'.to_d, disposal[:gain_loss]
    # Gross proceeds stay in their own column; only the allocation and the gain go net.
    assert_equal 25_000.to_d, disposal[:proceeds]
  end
end
