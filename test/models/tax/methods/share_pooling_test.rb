require 'test_helper'

class Tax::Methods::SharePoolingTest < ActiveSupport::TestCase
  setup do
    @sp = Tax::Methods::SharePooling.new
  end

  test 'same-day matching takes priority' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1, 10, 0), tx_id: 'sell-1', exchange: 'binance' },
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 48_000.to_d, transacted_at: Time.utc(2024, 6, 1, 14, 0), tx_id: 'buy-2', exchange: 'binance' }
    ]

    disposals = @sp.calculate(transactions)

    assert_equal 1, disposals.size
    # Same-day buy at 48k should be matched, not the older lot at 20k
    assert_equal 48_000.to_d, disposals.first[:cost_basis]
    assert_equal 2_000.to_d, disposals.first[:gain_loss]
    assert_includes disposals.first[:matching_rule], 'same_day'
  end

  test '30-day bed and breakfast matching' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'sell-1', exchange: 'binance' },
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 45_000.to_d, transacted_at: Time.utc(2024, 6, 15), tx_id: 'buy-2', exchange: 'binance' }
    ]

    disposals = @sp.calculate(transactions)

    assert_equal 1, disposals.size
    # 30-day forward buy at 45k matched (within 30 days of sell)
    assert_equal 45_000.to_d, disposals.first[:cost_basis]
    assert_includes disposals.first[:matching_rule], 'bed_and_breakfast'
  end

  test 'section 104 pool used when no same-day or 30-day match' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 2.to_d,
        fiat_value: 40_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :buy, base_currency: 'BTC', base_amount: 2.to_d,
        fiat_value: 60_000.to_d, transacted_at: Time.utc(2024, 2, 1), tx_id: 'buy-2', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'sell-1', exchange: 'binance' }
    ]

    disposals = @sp.calculate(transactions)

    assert_equal 1, disposals.size
    # Pool: (40000 + 60000) / 4 = 25000 per BTC
    assert_equal 25_000.to_d, disposals.first[:cost_basis]
    assert_equal 5_000.to_d, disposals.first[:gain_loss]
    assert_includes disposals.first[:matching_rule], 'section104'
  end
end
