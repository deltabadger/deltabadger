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
    refute_includes csv, I18n.t('tax_report.incomplete_banner_prefix', locale: :de)
    # Headers: date, acquisition_date, asset, amount, proceeds, cost_basis, gain_loss, ...
    row = CSV.parse(csv, headers: true).find { |r| r[2] == 'ETH' } # asset column
    assert_equal 10_000.to_d, row[5].to_d  # cost basis chained from the BTC purchase
    assert_equal 60_000.to_d, row[6].to_d  # gain 70k - 10k, not 70k - market-at-swap
    assert_equal 'false', row[I18n.t('tax_report.headers.data_incomplete', locale: :de)]
  end

  test 'missing price produces a banner row and flags the disposal, not silent zeros' do
    user = create(:user)
    exchange = create(:binance_exchange)
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :buy, base_currency: 'ZZZUNKNOWN',
                               base_amount: 1, transacted_at: Time.utc(2024, 2, 1), tx_id: 'q1')
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :sell, base_currency: 'ZZZUNKNOWN',
                               base_amount: 1, transacted_at: Time.utc(2024, 3, 1), tx_id: 'q2')
    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv
    rows = CSV.parse(csv, headers: true)
    assert_includes rows.first[0], I18n.t('tax_report.incomplete_banner_prefix', locale: :de)
    disposal = rows.find { |r| r[2] == 'ZZZUNKNOWN' }
    assert_equal 'true', disposal[I18n.t('tax_report.headers.data_incomplete', locale: :de)]
  end

  # The flag has to survive on the LOT, not just on the entry that carried the missing price:
  # here only the buy is unpriced, so a disposal-level check alone would let this sale look clean.
  test 'an unpriced acquisition keeps its lot flagged so a later fully priced sale is still incomplete' do
    user = create(:user)
    exchange = create(:binance_exchange)
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :buy, base_currency: 'ZZZUNKNOWN',
                               base_amount: 1, transacted_at: Time.utc(2024, 2, 1), tx_id: 'q1')
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :sell, base_currency: 'ZZZUNKNOWN',
                               base_amount: 1, quote_currency: 'EUR', quote_amount: 1_000,
                               transacted_at: Time.utc(2024, 3, 1), tx_id: 'q2')

    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv
    disposal = CSV.parse(csv, headers: true).find { |row| row[2] == 'ZZZUNKNOWN' }

    assert_equal 1_000.to_d, disposal[4].to_d # the sale itself priced fine, so the flag came off the lot
    assert_equal 'true', disposal[I18n.t('tax_report.headers.data_incomplete', locale: :de)]
  end

  test 'cold storage sweep is not a taxable event' do
    # Buy 1 BTC for 10 000 EUR on t0, withdraw all of it 10 days later to a wallet we do not sync.
    # Before this fix the withdrawal was a disposal and fabricated a taxable gain out of nothing.
    user = create(:user)
    exchange = create(:binance_exchange)
    t0 = Time.utc(2024, 1, 10)
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :buy, base_currency: 'BTC',
                               base_amount: 1, quote_currency: 'EUR', quote_amount: 10_000,
                               transacted_at: t0, tx_id: 'w1')
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :withdrawal, base_currency: 'BTC',
                               base_amount: 1, transacted_at: t0 + 10.days, tx_id: 'w2')

    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv

    assert_empty(CSV.parse(csv, headers: true).select { |row| row[2] == 'BTC' })
    # ...and the unpriceable withdrawal raises no missing-price banner either: no engine reads that
    # value any more, so warning about it would only erode the banner's credibility.
    refute_includes csv, I18n.t('tax_report.incomplete_banner_prefix', locale: :de)
  end

  test 'linked transfer pair keeps basis and later sale uses it' do
    # Binance buy 1 BTC @ 10 000 EUR -> withdraw 1 BTC -> Kraken deposit 0.999 BTC (0.001 network fee)
    # -> sell the 0.999 for 30 000 EUR. Exactly one disposal, and its basis is the original purchase
    # price minus the fee slice — NOT the market value the deposit would have invented.
    user = create(:user)
    binance = create(:binance_exchange)
    kraken = create(:kraken_exchange)
    t0 = Time.utc(2024, 1, 10)
    AccountTransaction.create!(user: user, exchange: binance, entry_type: :buy, base_currency: 'BTC',
                               base_amount: 1, quote_currency: 'EUR', quote_amount: 10_000,
                               transacted_at: t0, tx_id: 'l1')
    withdrawal = AccountTransaction.create!(user: user, exchange: binance, entry_type: :withdrawal,
                                            base_currency: 'BTC', base_amount: 1,
                                            transacted_at: t0 + 10.days, tx_id: 'l2')
    deposit = AccountTransaction.create!(user: user, exchange: kraken, entry_type: :deposit,
                                         base_currency: 'BTC', base_amount: '0.999'.to_d,
                                         transacted_at: t0 + 10.days + 1.hour, tx_id: 'l3')
    withdrawal.update!(linked_transaction_id: deposit.id)
    AccountTransaction.create!(user: user, exchange: kraken, entry_type: :sell, base_currency: 'BTC',
                               base_amount: '0.999'.to_d, quote_currency: 'EUR', quote_amount: 30_000,
                               transacted_at: t0 + 40.days, tx_id: 'l4')

    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv
    disposals = CSV.parse(csv, headers: true).select { |row| row[2] == 'BTC' }

    assert_equal 1, disposals.size          # exactly one disposal: the sale
    assert_equal 'l4', disposals.first[11]  # ...and it is the sale, not the transfer
    assert_equal '0.999'.to_d * 10_000, disposals.first[5].to_d.round(1) # basis from the original buy, minus the fee slice
    # Half the point of not disposing on transfer: the clock still runs from the original purchase,
    # which is what the German one-year §23 exemption is measured against.
    assert_equal '2024-01-10T00:00:00Z', disposals.first[1]
    assert_equal '40', disposals.first[8]
  end

  test 'an unlinked deposit keeps market-value basis but flags the later sale as assumed' do
    # We cannot see where these coins came from. Zero basis would fabricate a maximal gain, so we
    # assume market value at the deposit — and say so, via the same basis_assumed -> data_incomplete
    # channel an unpriced acquisition uses.
    user = create(:user)
    exchange = create(:binance_exchange)
    deposit_at = Time.utc(2024, 1, 10)
    HistoricalPrice.create!(asset: 'BTC', currency: 'EUR', date: deposit_at.to_date, price: 20_000.to_d)
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :deposit, base_currency: 'BTC',
                               base_amount: 1, transacted_at: deposit_at, tx_id: 'd1')
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :sell, base_currency: 'BTC',
                               base_amount: 1, quote_currency: 'EUR', quote_amount: 30_000,
                               transacted_at: deposit_at + 40.days, tx_id: 'd2')

    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv
    row = CSV.parse(csv, headers: true).find { |csv_row| csv_row[2] == 'BTC' }

    assert_equal 20_000.to_d, row[5].to_d
    assert_equal 'true', row[I18n.t('tax_report.headers.data_incomplete', locale: :de)]
  end
end
