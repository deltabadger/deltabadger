require 'test_helper'

class Tax::Methods::Fifo4WeekTest < ActiveSupport::TestCase
  setup do
    @engine = Tax::Methods::Fifo4Week.new
  end

  test '4-week rule matches recent acquisition instead of FIFO' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-old', exchange: 'binance' },
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 48_000.to_d, transacted_at: Time.utc(2024, 6, 10), tx_id: 'buy-recent', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 15), tx_id: 'sell-1', exchange: 'binance' }
    ]

    disposals = @engine.calculate(transactions)

    assert_equal 1, disposals.size
    # 4-week rule: Jun 10 buy is within 28 days of Jun 15 sell → match it
    assert_equal 48_000.to_d, disposals.first[:cost_basis]
    assert_equal '4_week_rule', disposals.first[:matching_rule]
    assert_equal 5, disposals.first[:holding_days]
  end

  test 'falls back to FIFO when no recent acquisition' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-old', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 15), tx_id: 'sell-1', exchange: 'binance' }
    ]

    disposals = @engine.calculate(transactions)

    assert_equal 1, disposals.size
    assert_equal 20_000.to_d, disposals.first[:cost_basis]
    assert_equal 'fifo', disposals.first[:matching_rule]
  end

  test 'period column: initial for Jan-Nov, later for December' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 2.to_d,
        fiat_value: 40_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 15), tx_id: 'sell-jun', exchange: 'binance' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 55_000.to_d, transacted_at: Time.utc(2024, 12, 10), tx_id: 'sell-dec', exchange: 'binance' }
    ]

    disposals = @engine.calculate(transactions)

    assert_equal 2, disposals.size
    assert_equal 'initial', disposals[0][:period]
    assert_equal 'later', disposals[1][:period]
  end
end
