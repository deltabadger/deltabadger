require 'test_helper'

class Tax::Methods::WealthSnapshotTest < ActiveSupport::TestCase
  setup do
    @engine = Tax::Methods::WealthSnapshot.new
    @price_service = mock('price_service')
  end

  test 'builds portfolio snapshot from transactions' do
    @price_service.stubs(:price_at).with(asset: 'BTC', currency: 'EUR', timestamp: anything).returns(50_000.to_d)
    @price_service.stubs(:convert_fiat).returns(1.to_d)

    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d, transacted_at: Time.utc(2025, 3, 1) },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 0.5.to_d, transacted_at: Time.utc(2025, 6, 1) }
    ]

    results = @engine.calculate(transactions, price_service: @price_service, currency: 'EUR', year: 2026,
                                              wealth_tax: { 2026 => { allowance: 59_357, deemed_return: 0.0778, rate: 0.36 } })

    holdings = results.select { |r| r[:type] == :holding }
    assert_equal 1, holdings.size
    assert_equal 'BTC', holdings.first[:asset]
    assert_equal 0.5.to_d, holdings.first[:amount]
    assert_equal 25_000.0, holdings.first[:value]
  end

  test 'excludes transactions after snapshot date' do
    @price_service.stubs(:price_at).returns(50_000.to_d)

    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d, transacted_at: Time.utc(2025, 6, 1) },
      { entry_type: :buy, base_currency: 'ETH', base_amount: 10.to_d, transacted_at: Time.utc(2026, 3, 1) }
    ]

    # Jan 1 2026 snapshot: only BTC buy (Jun 2025) included, ETH (Mar 2026) excluded
    results = @engine.calculate(transactions, price_service: @price_service, currency: 'EUR', year: 2026)

    holdings = results.select { |r| r[:type] == :holding }
    assert_equal 1, holdings.size
    assert_equal 'BTC', holdings.first[:asset]
  end

  test 'end of year snapshot uses Dec 31' do
    @price_service.stubs(:price_at).returns(50_000.to_d)

    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d, transacted_at: Time.utc(2025, 6, 1) },
      { entry_type: :buy, base_currency: 'ETH', base_amount: 10.to_d, transacted_at: Time.utc(2025, 11, 1) }
    ]

    results = @engine.calculate(transactions, price_service: @price_service, currency: 'CHF',
                                              year: 2025, snapshot_date: :end_of_year, summary_only_total: true)

    holdings = results.select { |r| r[:type] == :holding }
    assert_equal 2, holdings.size # Both BTC and ETH included (before Dec 31 2025)

    summary = results.select { |r| r[:type] == :summary }
    assert_equal 1, summary.size # Only total_value, no tax calc
  end

  test 'summary includes tax calculation for NL' do
    @price_service.stubs(:price_at).returns(100_000.to_d)

    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d, transacted_at: Time.utc(2025, 1, 1) }
    ]

    results = @engine.calculate(transactions, price_service: @price_service, currency: 'EUR', year: 2026,
                                              wealth_tax: { 2026 => { allowance: 59_357, deemed_return: 0.0778, rate: 0.36 } })

    summary = results.select { |r| r[:type] == :summary }
    total = summary.find { |r| r[:label] == 'total_value' }
    taxable = summary.find { |r| r[:label] == 'taxable_wealth' }

    assert_equal 100_000.0, total[:value]
    assert_equal 40_643.0, taxable[:value] # 100000 - 59357
  end
end
