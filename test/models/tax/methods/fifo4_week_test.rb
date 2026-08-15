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

  test 'buy fee increases cost basis for a 4-week match' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 20_000.to_d, fee_currency: 'EUR', fee_fiat_value: 50.to_d,
        transacted_at: Time.utc(2024, 6, 10), tx_id: 'fee-buy' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 15), tx_id: 'fee-sell' }
    ]

    disposal = @engine.calculate(transactions).first

    assert_equal 20_050.to_d, disposal[:cost_basis]
  end

  test 'split before disposal preserves cost basis' do
    transactions = [
      { entry_type: :buy, base_currency: 'NVDA', base_amount: 10.to_d,
        fiat_value: 1_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'split-buy',
        exchange: 'alpaca' },
      { entry_type: :adjustment, base_currency: 'NVDA', base_amount: 90.to_d,
        fiat_value: 0.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'split-adjustment',
        exchange: 'alpaca' },
      { entry_type: :sell, base_currency: 'NVDA', base_amount: 50.to_d,
        fiat_value: 1_000.to_d, transacted_at: Time.utc(2024, 7, 1), tx_id: 'split-sell',
        exchange: 'alpaca' }
    ]

    disposal = Tax::Methods::Fifo4Week.new.calculate(transactions).first

    assert_equal 500.to_d, disposal[:cost_basis]
  end

  test 'the 4-week rule still matches a lot a split rescaled' do
    transactions = [
      { entry_type: :buy, base_currency: 'NVDA', base_amount: 10.to_d,
        fiat_value: 1_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'recent-buy',
        exchange: 'alpaca' },
      { entry_type: :adjustment, base_currency: 'NVDA', base_amount: 90.to_d,
        fiat_value: 0.to_d, transacted_at: Time.utc(2024, 6, 10), tx_id: 'recent-adjustment',
        exchange: 'alpaca' },
      { entry_type: :sell, base_currency: 'NVDA', base_amount: 50.to_d,
        fiat_value: 1_000.to_d, transacted_at: Time.utc(2024, 6, 20), tx_id: 'recent-sell',
        exchange: 'alpaca' }
    ]

    disposal = Tax::Methods::Fifo4Week.new.calculate(transactions).first

    # The split kept the lot's own date, so the sale 19 days later still matches it: 50 of the
    # 100 post-split units at 10 each.
    assert_equal '4_week_rule', disposal[:matching_rule]
    assert_equal Time.utc(2024, 6, 1), disposal[:acquisition_date]
    assert_equal 19, disposal[:holding_days]
    assert_equal 500.to_d, disposal[:cost_basis]
  end
  # Two equal fills of one order are byte-identical lot hashes. `Array#delete` deletes by `==`, so
  # consuming one of them destroyed both and a whole lot's basis vanished.
  test 'a 4-week match consumes one of two identical lots, not both' do
    buy_at = Time.utc(2024, 6, 1)
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d, fiat_value: 20_000.to_d,
        transacted_at: buy_at, tx_id: 'fill-a', exchange: 'kraken' },
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d, fiat_value: 20_000.to_d,
        transacted_at: buy_at, tx_id: 'fill-b', exchange: 'kraken' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d, fiat_value: 25_000.to_d,
        transacted_at: Time.utc(2024, 6, 10), tx_id: 'sell-first', exchange: 'kraken' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d, fiat_value: 26_000.to_d,
        transacted_at: Time.utc(2024, 6, 20), tx_id: 'sell-second', exchange: 'kraken' }
    ]

    disposals = @engine.calculate(transactions)

    assert_equal 20_000.to_d, disposals[0][:cost_basis]
    assert_equal 20_000.to_d, disposals[1][:cost_basis]
    assert disposals[1][:cost_basis_complete]
  end
end
