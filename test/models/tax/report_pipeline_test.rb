require 'test_helper'

# End-to-end pipeline coverage for real AccountTransaction records through Tax::Report.
class Tax::ReportPipelineTest < ActiveSupport::TestCase
  test 'German report capitalises a fiat acquisition fee end to end' do
    user = create(:user)
    exchange = create(:binance_exchange)
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :buy, base_currency: 'BTC',
                               base_amount: 1, quote_currency: 'EUR', quote_amount: 10_000,
                               fee_currency: 'EUR', fee_amount: 50,
                               transacted_at: Time.utc(2024, 1, 1), tx_id: 'fee-pipeline-buy')
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :sell, base_currency: 'BTC',
                               base_amount: 1, quote_currency: 'EUR', quote_amount: 30_000,
                               transacted_at: Time.utc(2024, 6, 1), tx_id: 'fee-pipeline-sell')

    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv
    disposal = CSV.parse(csv, headers: true).find { |row| row[2] == 'BTC' }

    assert_equal 10_050.to_d, disposal[5].to_d
  end

  test 'German report attaches a Kraken quote-row fee to the grouped BTC acquisition' do
    user = create(:user)
    exchange = create(:kraken_exchange)
    buy_at = Time.utc(2024, 1, 10)
    HistoricalPrice.create!(asset: 'BTC', currency: 'EUR', date: buy_at.to_date, price: 20_000.to_d)
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :buy, base_currency: 'BTC',
                               base_amount: '0.5'.to_d, group_id: 'R1',
                               transacted_at: buy_at, tx_id: 'kraken-btc')
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :sell, base_currency: 'EUR',
                               base_amount: 10_000, group_id: 'R1', fee_currency: 'EUR', fee_amount: 26,
                               transacted_at: buy_at, tx_id: 'kraken-eur')
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :sell, base_currency: 'BTC',
                               base_amount: '0.5'.to_d, quote_currency: 'EUR', quote_amount: 30_000,
                               transacted_at: Time.utc(2024, 6, 1), tx_id: 'kraken-btc-sale')

    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv
    rows = CSV.parse(csv, headers: true)

    assert_equal 10_026.to_d, rows.find { |row| row[2] == 'BTC' }[5].to_d
    assert_empty(rows.select { |row| row[2] == 'EUR' }) # the donor row funded the fee, it did not dispose of anything
  end

  # Kraken books a trade as one signed ledger row per asset, so a EUR-funded BTC buy arrives as a
  # `sell` of EUR. Priced at 1.0 against lots the user never had, that fiat leg used to be reported
  # as a disposal of the entire funding amount at zero cost.
  test 'German report ignores Kraken fiat ledger rows without losing the attributed fee' do
    user = create(:user)
    exchange = create(:kraken_exchange)
    buy_at = Time.utc(2024, 1, 10)
    sell_at = Time.utc(2024, 6, 1)
    HistoricalPrice.create!(asset: 'BTC', currency: 'EUR', date: buy_at.to_date, price: 20_000.to_d)
    [
      { entry_type: :buy, base_currency: 'BTC', base_amount: '0.5'.to_d, group_id: 'R1',
        transacted_at: buy_at, tx_id: 'kraken-de-btc-buy' },
      { entry_type: :sell, base_currency: 'EUR', base_amount: 10_000.to_d, group_id: 'R1',
        fee_currency: 'EUR', fee_amount: 26.to_d, transacted_at: buy_at, tx_id: 'kraken-de-eur-sell' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: '0.5'.to_d,
        quote_currency: 'EUR', quote_amount: 30_000.to_d, group_id: 'R2',
        transacted_at: sell_at, tx_id: 'kraken-de-btc-sell' },
      { entry_type: :buy, base_currency: 'EUR', base_amount: 30_000.to_d, group_id: 'R2',
        transacted_at: sell_at, tx_id: 'kraken-de-eur-buy' }
    ].each { |attrs| AccountTransaction.create!(user: user, exchange: exchange, **attrs) }

    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv
    rows = CSV.parse(csv, headers: true)
    btc_disposals = rows.select { |row| row[2] == 'BTC' }

    assert_equal 1, btc_disposals.size
    assert_equal 10_026.to_d, btc_disposals.first[5].to_d # Kraken's EUR-row fee is still capitalised
    assert_equal 19_974.to_d, btc_disposals.first[6].to_d
    assert_empty(rows.select { |row| row[2] == 'EUR' })
  end

  # Kraken books the fee of a SALE on the EUR row the proceeds arrive on. Restricting the fee move
  # to acquisitions left that fee reaching no engine at all — deductible under §20(4) EStG.
  test 'German report deducts a Kraken quote-row fee booked on a sale' do
    user = create(:user)
    exchange = create(:kraken_exchange)
    buy_at = Time.utc(2024, 1, 10)
    sell_at = Time.utc(2024, 6, 1)
    HistoricalPrice.create!(asset: 'BTC', currency: 'EUR', date: buy_at.to_date, price: 20_000.to_d)
    [
      { entry_type: :buy, base_currency: 'BTC', base_amount: '0.5'.to_d, group_id: 'R1',
        transacted_at: buy_at, tx_id: 'kraken-sellfee-btc-buy' },
      { entry_type: :sell, base_currency: 'EUR', base_amount: 10_000.to_d, group_id: 'R1',
        transacted_at: buy_at, tx_id: 'kraken-sellfee-eur-sell' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: '0.5'.to_d,
        quote_currency: 'EUR', quote_amount: 30_000.to_d, group_id: 'R2',
        transacted_at: sell_at, tx_id: 'kraken-sellfee-btc-sell' },
      { entry_type: :buy, base_currency: 'EUR', base_amount: 30_000.to_d, group_id: 'R2',
        fee_currency: 'EUR', fee_amount: 78.to_d, transacted_at: sell_at, tx_id: 'kraken-sellfee-eur-buy' }
    ].each { |attrs| AccountTransaction.create!(user: user, exchange: exchange, **attrs) }

    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv
    disposal = CSV.parse(csv, headers: true).find { |row| row[2] == 'BTC' }

    assert_equal 78.to_d, disposal[9].to_d
    assert_equal 19_922.to_d, disposal[6].to_d
  end

  # Kraken settles in AUD, CAD, JPY and AED too. Off the fiat list, the funding leg of an AUD-funded
  # buy arrives as a `sell` of AUD priced at 0 against lots the user never had.
  test 'German report ignores a Kraken AUD settlement row' do
    user = create(:user)
    exchange = create(:kraken_exchange)
    buy_at = Time.utc(2024, 1, 10)
    HistoricalPrice.create!(asset: 'BTC', currency: 'EUR', date: buy_at.to_date, price: 20_000.to_d)
    [
      { entry_type: :buy, base_currency: 'BTC', base_amount: '0.5'.to_d, group_id: 'R1',
        transacted_at: buy_at, tx_id: 'kraken-aud-btc-buy' },
      { entry_type: :sell, base_currency: 'AUD', base_amount: 16_000.to_d, group_id: 'R1',
        transacted_at: buy_at, tx_id: 'kraken-aud-fiat-sell' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: '0.5'.to_d,
        quote_currency: 'EUR', quote_amount: 30_000.to_d,
        transacted_at: Time.utc(2024, 6, 1), tx_id: 'kraken-aud-btc-sell' }
    ].each { |attrs| AccountTransaction.create!(user: user, exchange: exchange, **attrs) }

    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv

    assert_empty(CSV.parse(csv, headers: true).select { |row| row[2] == 'AUD' })
    refute_includes csv, I18n.t('tax_report.incomplete_banner_prefix', locale: :de)
  end

  # `insert_all`/`insert` bypass HistoricalPrice's presence validation, so a null in an upstream
  # prices array (nil.to_d == 0) can persist as a zero and be read back forever as a valid price.
  # The whole proceeds then become gain on a row that claims to be complete.
  test 'a stored zero price is treated as missing, not as a free acquisition' do
    user = create(:user)
    exchange = create(:binance_exchange)
    buy_at = Time.utc(2024, 1, 10)
    HistoricalPrice.create!(asset: 'BTC', currency: 'EUR', date: buy_at.to_date, price: 0.to_d)
    [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        transacted_at: buy_at, tx_id: 'zero-price-buy' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: 1.to_d,
        quote_currency: 'EUR', quote_amount: 30_000.to_d,
        transacted_at: Time.utc(2024, 6, 1), tx_id: 'zero-price-sell' }
    ].each { |attrs| AccountTransaction.create!(user: user, exchange: exchange, **attrs) }

    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv

    assert_includes csv, I18n.t('tax_report.incomplete_banner_prefix', locale: :de)
  end

  # PVCT has no lots: the EUR leg of a sale would swell the portfolio-wide acquisition-cost pool,
  # and the EUR leg of a purchase would drain it as a cession, corrupting every later disposal.
  test 'French report ignores Kraken fiat ledger rows in the acquisition cost pool' do
    user = create(:user)
    exchange = create(:kraken_exchange)
    buy_at = Time.utc(2024, 1, 10)
    sell_at = Time.utc(2024, 6, 1)
    HistoricalPrice.create!(asset: 'BTC', currency: 'EUR', date: buy_at.to_date, price: 20_000.to_d)
    HistoricalPrice.create!(asset: 'BTC', currency: 'EUR', date: sell_at.to_date, price: 60_000.to_d)
    [
      { entry_type: :buy, base_currency: 'BTC', base_amount: '0.5'.to_d, group_id: 'R1',
        transacted_at: buy_at, tx_id: 'kraken-fr-btc-buy' },
      { entry_type: :sell, base_currency: 'EUR', base_amount: 10_000.to_d, group_id: 'R1',
        transacted_at: buy_at, tx_id: 'kraken-fr-eur-sell' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: '0.5'.to_d,
        quote_currency: 'EUR', quote_amount: 30_000.to_d, group_id: 'R2',
        transacted_at: sell_at, tx_id: 'kraken-fr-btc-sell' },
      { entry_type: :buy, base_currency: 'EUR', base_amount: 30_000.to_d, group_id: 'R2',
        transacted_at: sell_at, tx_id: 'kraken-fr-eur-buy' }
    ].each { |attrs| AccountTransaction.create!(user: user, exchange: exchange, **attrs) }

    csv = Tax::Report.new(country: 'FR', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv
    rows = CSV.parse(csv, headers: true)
    btc_cessions = rows.select { |row| row[1] == 'BTC' }

    assert_equal 1, btc_cessions.size
    assert_equal 30_000.to_d, btc_cessions.first[3].to_d
    assert_equal 10_000.to_d, btc_cessions.first[4].to_d
    assert_equal 20_000.to_d, btc_cessions.first[6].to_d
    assert_empty(rows.select { |row| row[1] == 'EUR' })
  end

  # A fiat base outside the report currency is still unused; trying to price it used to fabricate
  # a missing-price warning and banner an otherwise complete report.
  test 'German report ignores a Kraken USD ledger row without a missing-price warning' do
    user = create(:user)
    exchange = create(:kraken_exchange)
    buy_at = Time.utc(2024, 1, 10)
    HistoricalPrice.create!(asset: 'BTC', currency: 'EUR', date: buy_at.to_date, price: 20_000.to_d)
    [
      { entry_type: :buy, base_currency: 'BTC', base_amount: '0.5'.to_d, group_id: 'R1',
        transacted_at: buy_at, tx_id: 'kraken-usd-btc-buy' },
      { entry_type: :sell, base_currency: 'USD', base_amount: 10_000.to_d, group_id: 'R1',
        transacted_at: buy_at, tx_id: 'kraken-usd-fiat-sell' },
      { entry_type: :sell, base_currency: 'BTC', base_amount: '0.5'.to_d,
        quote_currency: 'EUR', quote_amount: 30_000.to_d,
        transacted_at: Time.utc(2024, 6, 1), tx_id: 'kraken-usd-btc-sell' }
    ].each { |attrs| AccountTransaction.create!(user: user, exchange: exchange, **attrs) }

    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv

    assert_empty(CSV.parse(csv, headers: true).select { |row| row[2] == 'USD' })
    refute_includes csv, I18n.t('tax_report.incomplete_banner_prefix', locale: :de)
  end

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

  test 'unlinked deposit adds a warning naming its asset and date' do
    user = create(:user)
    exchange = create(:binance_exchange)
    deposit_at = Time.utc(2023, 12, 10)
    HistoricalPrice.create!(asset: 'BTC', currency: 'EUR', date: deposit_at.to_date, price: 20_000.to_d)
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :deposit, base_currency: 'BTC',
                               base_amount: 1, transacted_at: deposit_at, tx_id: 'warning-1')
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :sell, base_currency: 'BTC',
                               base_amount: 1, quote_currency: 'EUR', quote_amount: 30_000,
                               transacted_at: Time.utc(2024, 2, 1), tx_id: 'warning-2')

    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv
    warning = I18n.t('tax_report.warnings.deposit_basis_assumed', locale: :de)

    assert_includes csv, "WARNING: #{warning}:"
    assert CSV.parse(csv).any? { |row| row.first&.start_with?('BTC ') && row.first.end_with?(' 2023-12-10') },
           'expected the warning block to name the BTC deposit and its original date'
  end

  test 'a fiat deposit is not named in the assumed-basis warning' do
    user = create(:user)
    exchange = create(:binance_exchange)
    deposit_at = Time.utc(2023, 12, 10)
    HistoricalPrice.create!(asset: 'BTC', currency: 'EUR', date: deposit_at.to_date, price: 20_000.to_d)
    # Bank funding: nothing to link it to and no cost basis to assume.
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :deposit, base_currency: 'EUR',
                               base_amount: 5000, transacted_at: deposit_at, tx_id: 'fiat-warning-1')
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :deposit, base_currency: 'BTC',
                               base_amount: 1, transacted_at: deposit_at, tx_id: 'fiat-warning-2')

    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv
    named = CSV.parse(csv).map(&:first).compact.grep(/\A(BTC|EUR) /)

    assert_equal 1, named.size, "expected only the BTC deposit to be named, got #{named.inspect}"
    assert named.first.start_with?('BTC ')
  end

  test 'linked transfer deposit does not add an assumed-basis warning' do
    user = create(:user)
    binance = create(:binance_exchange)
    kraken = create(:kraken_exchange)
    withdrawal = AccountTransaction.create!(user: user, exchange: binance, entry_type: :withdrawal,
                                            base_currency: 'BTC', base_amount: 1,
                                            transacted_at: Time.utc(2024, 1, 10), tx_id: 'linked-warning-1')
    deposit = AccountTransaction.create!(user: user, exchange: kraken, entry_type: :deposit,
                                         base_currency: 'BTC', base_amount: 1,
                                         transacted_at: Time.utc(2024, 1, 10, 1), tx_id: 'linked-warning-2')
    withdrawal.update!(linked_transaction_id: deposit.id)

    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv

    refute_includes csv, I18n.t('tax_report.warnings.deposit_basis_assumed', locale: :de)
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

  test 'the report lists income received with a per-type total' do
    user = create(:user)
    exchange = create(:binance_exchange)
    HistoricalPrice.create!(asset: 'BTC', currency: 'EUR', date: Date.new(2024, 3, 1), price: 20_000.to_d)
    HistoricalPrice.create!(asset: 'BTC', currency: 'EUR', date: Date.new(2024, 4, 1), price: 20_000.to_d)
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :buy, base_currency: 'BTC',
                               base_amount: 1, quote_currency: 'EUR', quote_amount: 10_000,
                               transacted_at: Time.utc(2024, 1, 1))
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :sell, base_currency: 'BTC',
                               base_amount: 1, quote_currency: 'EUR', quote_amount: 30_000,
                               transacted_at: Time.utc(2024, 6, 1), tx_id: 'income-sell')
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :staking_reward,
                               base_currency: 'BTC', base_amount: '0.1'.to_d,
                               transacted_at: Time.utc(2024, 3, 1))
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :staking_reward,
                               base_currency: 'BTC', base_amount: '0.1'.to_d,
                               transacted_at: Time.utc(2024, 4, 1))
    # A second income type in the same report: the totals must stay separate, not merge into one.
    HistoricalPrice.create!(asset: 'UNI', currency: 'EUR', date: Date.new(2024, 5, 1), price: 10.to_d)
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :airdrop,
                               base_currency: 'UNI', base_amount: 50.to_d,
                               transacted_at: Time.utc(2024, 5, 1))

    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv
    rows = CSV.parse(csv)
    income_header = I18n.t('tax_report.income.header', locale: :de)
    staking_label = I18n.t('tax_report.income.types.staking_reward', locale: :de)
    airdrop_label = I18n.t('tax_report.income.types.airdrop', locale: :de)

    assert_includes rows, [income_header]
    income_rows = rows.select { |row| row[1] == staking_label && row[2] == 'BTC' }
    assert_equal 2, income_rows.size
    amounts = income_rows.map { |row| row[3].to_d }
    values = income_rows.map { |row| row[4].to_d }
    currencies = income_rows.map { |row| row[5] }

    assert_equal %w[2024-03-01 2024-04-01], income_rows.map(&:first)
    assert_equal ['0.1'.to_d, '0.1'.to_d], amounts
    assert_equal [2_000.to_d, 2_000.to_d], values
    assert_equal %w[EUR EUR], currencies

    staking_total = I18n.t('tax_report.income.total', type: staking_label, locale: :de)
    airdrop_total = I18n.t('tax_report.income.total', type: airdrop_label, locale: :de)
    total_rows = rows.select { |row| [staking_total, airdrop_total].include?(row[1]) }
    total_labels = total_rows.map { |row| row[1] }
    total_values = total_rows.map { |row| row[4].to_d }

    assert_equal [staking_total, airdrop_total], total_labels
    assert_equal [4_000.to_d, 500.to_d], total_values

    disposal_rows = CSV.parse(csv, headers: true)
    disposal = disposal_rows.find do |row|
      row[I18n.t('tax_report.headers.tx_id', locale: :de)] == 'income-sell'
    end
    assert_equal 20_000.to_d, disposal[I18n.t('tax_report.headers.gain_loss', locale: :de)].to_d
  end

  test 'a USD-denominated dividend is reported as income' do
    user = create(:user)
    exchange = create(:alpaca_exchange)
    FxRate.create!(currency: 'USD', date: Date.new(2024, 5, 2), rate: '1.25'.to_d)
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :other_income,
                               base_currency: 'USD', base_amount: '12.5'.to_d,
                               transacted_at: Time.utc(2024, 5, 2), tx_id: 'usd-dividend')

    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv
    rows = CSV.parse(csv)
    other_income_label = I18n.t('tax_report.income.types.other_income', locale: :de)
    income_row = rows.find { |row| row[1] == other_income_label && row[2] == 'USD' }

    assert income_row, 'expected a USD other-income row'
    assert_equal 10.to_d, income_row&.[](4)&.to_d
    assert_equal 'EUR', income_row&.[](5)

    # Report#taxable_entries drops every fiat-base row before the engines, and income rows are
    # frequently fiat-base (Alpaca dividends/`INT`, Kraken `dividend`), so an income section built
    # from the filtered array would silently lose every dividend.
    disposal_block = rows.take_while(&:present?)
    disposal_csv = CSV.generate { |block| disposal_block.each { |row| block << row } }
    disposal_rows = CSV.parse(disposal_csv, headers: true)
    asset_header = I18n.t('tax_report.headers.asset', locale: :de)
    assert_empty(disposal_rows.select { |row| row[asset_header] == 'USD' })
  end

  # A fiat income row is the ONLY place a missing FX rate for it can surface: enrich short-circuits a
  # fiat base to zero without touching FX. Value the section before the banner is decided, or a report
  # whose only defect is that gap prints a clean header over a warning footer.
  test 'an unpriceable fiat income row reaches the incomplete banner' do
    user = create(:user)
    exchange = create(:alpaca_exchange)
    # No FxRate seeded on purpose: USD/EUR cannot be resolved for this date.
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :other_income,
                               base_currency: 'USD', base_amount: '12.5'.to_d,
                               transacted_at: Time.utc(2024, 5, 2), tx_id: 'unpriceable-dividend')

    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv
    rows = CSV.parse(csv)
    warnings_start = rows.index(["WARNING: #{I18n.t('tax_report.warnings.missing_prices', locale: :de)}:"])
    listed = rows[(warnings_start + 1)..].take_while { |row| row != [I18n.t('tax_report.warnings.upgrade_hint', locale: :de)] }
    banner = rows[1].first

    assert_includes listed.flatten, 'FX USD/EUR 2024-05-02'
    assert_equal "#{I18n.t('tax_report.incomplete_banner_prefix', locale: :de)}: " \
                 "#{I18n.t('tax_report.incomplete_banner', missing: listed.size, locale: :de)}", banner
  end

  test 'income received outside the report year is not listed' do
    user = create(:user)
    exchange = create(:binance_exchange)
    HistoricalPrice.create!(asset: 'BTC', currency: 'EUR', date: Date.new(2023, 3, 1), price: 20_000.to_d)
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :staking_reward,
                               base_currency: 'BTC', base_amount: '0.1'.to_d,
                               transacted_at: Time.utc(2023, 3, 1))

    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv

    refute_includes csv, I18n.t('tax_report.income.header', locale: :de)
  end

  test 'income is listed even when there are no disposals' do
    user = create(:user)
    exchange = create(:binance_exchange)
    HistoricalPrice.create!(asset: 'UNI', currency: 'EUR', date: Date.new(2024, 2, 2), price: 5.to_d)
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :airdrop,
                               base_currency: 'UNI', base_amount: 100.to_d,
                               transacted_at: Time.utc(2024, 2, 2))

    csv = Tax::Report.new(country: 'DE', year: 2024, transactions: AccountTransaction.for_user(user)).to_csv
    rows = CSV.parse(csv)
    income_header = I18n.t('tax_report.income.header', locale: :de)
    airdrop_label = I18n.t('tax_report.income.types.airdrop', locale: :de)

    assert_includes csv, I18n.t('tax_report.no_taxable_transactions', locale: :de)
    assert_includes rows, [income_header]
    income_row = rows.find { |row| row[1] == airdrop_label && row[2] == 'UNI' }
    assert_equal 500.to_d, income_row&.[](4)&.to_d
  end

  test 'the Swiss report discloses that income is taxed separately' do
    user = create(:user)
    exchange = create(:kraken_exchange)
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :buy, base_currency: 'BTC',
                               base_amount: 1.to_d, quote_currency: 'EUR', quote_amount: 10_000.to_d,
                               transacted_at: Time.utc(2024, 1, 1))
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: :staking_reward,
                               base_currency: 'BTC', base_amount: '0.1'.to_d,
                               transacted_at: Time.utc(2024, 3, 1))

    transactions = AccountTransaction.for_user(user)
    swiss_csv = Tax::Report.new(country: 'CH', year: 2024, transactions: transactions).to_csv
    german_csv = Tax::Report.new(country: 'DE', year: 2024, transactions: transactions).to_csv
    not_included = I18n.t('tax_report.income.not_included', locale: :de)
    income_header = I18n.t('tax_report.income.header', locale: :de)

    assert_includes swiss_csv, not_included
    refute_includes swiss_csv, income_header
    refute_includes german_csv, not_included
  end
end
