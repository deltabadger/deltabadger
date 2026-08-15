require 'test_helper'

# End-to-end pipeline coverage for real AccountTransaction records through Tax::Report.
class Tax::ReportPipelineTest < ActiveSupport::TestCase
  # AT: crypto->crypto not taxable; basis must chain through the swap via group_id.
  test 'austrian swap chains cost basis end to end' do
    user = create(:user)
    exchange = create(:binance_exchange)
    t0 = Time.utc(2024, 1, 10)
    swap_at = t0 + 30.days
    sell_at = t0 + 60.days
    FxRate.create!(currency: 'USD', date: t0.to_date, rate: 1.to_d) # 1:1 keeps arithmetic readable
    # Swap-date market prices: 20 ETH is worth 40 000 EUR here, so market value at the swap and
    # the 10 000 chained basis are different numbers. Seeded locally so no price is ever missing.
    HistoricalPrice.create!(asset: 'BTC', currency: 'EUR', date: swap_at.to_date, price: 40_000.to_d)
    HistoricalPrice.create!(asset: 'ETH', currency: 'EUR', date: swap_at.to_date, price: 2_000.to_d)
    [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1, quote_currency: 'EUR', quote_amount: 10_000, transacted_at: t0, tx_id: 'p1' },
      { entry_type: :swap_out, base_currency: 'BTC', base_amount: 1, quote_currency: nil, group_id: 'g1', transacted_at: swap_at, tx_id: 'p2' },
      { entry_type: :swap_in, base_currency: 'ETH', base_amount: 20, quote_currency: nil, group_id: 'g1', transacted_at: swap_at, tx_id: 'p3' },
      { entry_type: :sell, base_currency: 'ETH', base_amount: 20, quote_currency: 'EUR', quote_amount: 70_000, transacted_at: sell_at, tx_id: 'p4' }
    ].each { |attrs| AccountTransaction.create!(user: user, exchange: exchange, **attrs) }

    csv = Tax::Report.new(country: 'AT', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv
    # Headers: date, acquisition_date, asset, amount, proceeds, cost_basis, gain_loss, ...
    row = CSV.parse(csv, headers: true).find { |r| r[2] == 'ETH' } # asset column
    assert_equal 10_000.to_d, row[5].to_d  # cost basis chained from the BTC purchase
    assert_equal 60_000.to_d, row[6].to_d  # gain 70k - 10k, not 70k - market-at-swap
  end
end
