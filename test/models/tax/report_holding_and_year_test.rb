require 'test_helper'

# Holding periods, tax years and in-kind trade fees, end to end through Tax::Report.
class Tax::ReportHoldingAndYearTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:kraken_exchange)
  end

  # --- A sale across lots of different ages is one row per term ---

  test 'a US sale spanning a long and a short lot is split into one row per term' do
    trade(:buy, 1, 10_000, Time.utc(2022, 1, 10))
    trade(:buy, 1, 40_000, Time.utc(2024, 1, 10))
    trade(:sell, 2, 120_000, Time.utc(2024, 6, 1))

    rows = disposals('US', 2024)

    assert_equal 2, rows.size
    long, short = rows
    assert_equal ['2022-01-10', 1.to_d, 60_000.to_d, 10_000.to_d, 50_000.to_d, 'Long-term'],
                 [long[1][0, 10], long[3].to_d, long[4].to_d, long[5].to_d, long[6].to_d, long['Term']]
    assert_equal ['2024-01-10', 1.to_d, 60_000.to_d, 40_000.to_d, 20_000.to_d, 'Short-term'],
                 [short[1][0, 10], short[3].to_d, short[4].to_d, short[5].to_d, short[6].to_d, short['Term']]
  end

  test 'a German sale spanning an exempt and a taxable lot is split, the fee shared pro rata' do
    trade(:buy, 1, 10_000, Time.utc(2022, 1, 10), currency: 'EUR')
    trade(:buy, 3, 90_000, Time.utc(2024, 1, 10), currency: 'EUR')
    trade(:sell, 4, 160_000, Time.utc(2024, 6, 1), currency: 'EUR', fee: 400)

    rows = disposals('DE', 2024)
    exempt = I18n.t('tax_report.headers.tax_exempt', locale: :de)

    assert_equal(%w[true false], rows.map { |row| row[exempt] })
    assert_equal([40_000.to_d, 120_000.to_d], rows.map { |row| row[4].to_d })
    assert_equal([100.to_d, 300.to_d], rows.map { |row| row[9].to_d })
    assert_equal([29_900.to_d, 29_700.to_d], rows.map { |row| row[6].to_d })
  end

  test 'a sale whose lots share one term stays one row' do
    trade(:buy, 1, 10_000, Time.utc(2024, 1, 10))
    trade(:buy, 1, 20_000, Time.utc(2024, 2, 10))
    trade(:sell, 2, 50_000, Time.utc(2024, 6, 1))

    assert_equal 1, disposals('US', 2024).size
  end

  # --- Long-held means after the calendar anniversary, in the user's zone ---

  test 'a sale on the anniversary across 29 February is still short-term in the US' do
    trade(:buy, 1, 10_000, Time.utc(2023, 3, 1, 12))
    trade(:sell, 1, 20_000, Time.utc(2024, 3, 1, 12)) # 366 days, but not more than a year

    assert_equal 'Short-term', disposals('US', 2024).first['Term']
  end

  test 'a sale the day after the anniversary is long-term even under 365 full days by the clock' do
    trade(:buy, 1, 10_000, Time.utc(2022, 1, 10, 20))
    trade(:sell, 1, 20_000, Time.utc(2023, 1, 11, 8)) # 365.5 days

    assert_equal 'Long-term', disposals('US', 2023).first['Term']
  end

  test 'the anniversary is taken in the user time zone' do
    trade(:buy, 1, 10_000, Time.utc(2022, 1, 11, 3)) # 10 January, 22:00 in New York
    trade(:sell, 1, 20_000, Time.utc(2023, 1, 11, 14))

    assert_equal 'Short-term', disposals('US', 2023).first['Term']
    assert_equal 'Long-term', disposals('US', 2023, time_zone: 'America/New_York').first['Term']
  end

  test 'Portugal exempts a holding of 365 days, Germany only one of more than a year' do
    trade(:buy, 1, 10_000, Time.utc(2023, 3, 1), currency: 'EUR')
    trade(:sell, 1, 20_000, Time.utc(2024, 2, 29), currency: 'EUR') # 365 days, before the anniversary

    assert_equal 'true', disposals('PT', 2024).first[I18n.t('tax_report.headers.tax_exempt', locale: :pt)]
    assert_equal 'false', disposals('DE', 2024).first[I18n.t('tax_report.headers.tax_exempt', locale: :de)]
  end

  test 'the Czech time test passes the day after the third anniversary' do
    trade(:buy, 1, 1_000_000, Time.utc(2021, 1, 10, 20), currency: 'CZK')
    trade(:sell, 1, 2_000_000, Time.utc(2024, 1, 11, 8), currency: 'CZK') # 1095.5 days

    row = disposals('CZ', 2024).first

    assert_equal 'časový test', row.fields.last
  end

  test 'Slovakia applies the long-term rate per row of a split sale' do
    trade(:buy, 1, 10_000, Time.utc(2022, 1, 10), currency: 'EUR')
    trade(:buy, 1, 40_000, Time.utc(2024, 1, 10), currency: 'EUR')
    trade(:sell, 2, 120_000, Time.utc(2024, 6, 1), currency: 'EUR')

    rate = I18n.t('tax_report.headers.tax_rate', locale: :sk)

    assert_equal(%w[7% 19%], disposals('SK', 2024).map { |row| row[rate] })
  end

  test 'Austrian old stock is decided per lot of a sale that also takes new stock' do
    trade(:buy, 1, 5_000, Time.utc(2020, 6, 1), currency: 'EUR')
    trade(:buy, 1, 35_000, Time.utc(2024, 1, 10), currency: 'EUR')
    trade(:sell, 2, 100_000, Time.utc(2024, 6, 1), currency: 'EUR')

    old_stock = I18n.t('tax_report.headers.old_stock', locale: :de)

    assert_equal(%w[true false], disposals('AT', 2024).map { |row| row[old_stock] })
  end

  # --- The tax year is the year in the user's zone ---

  test 'a New Year\'s Eve sale in Los Angeles belongs to the year it was made there' do
    trade(:buy, 1, 10_000, Time.utc(2024, 1, 10))
    trade(:sell, 1, 20_000, Time.utc(2025, 1, 1, 5)) # 31 December 2024, 21:00 in Los Angeles

    assert_equal 1, disposals('US', 2024, time_zone: 'America/Los_Angeles').size
    assert_empty disposals('US', 2025, time_zone: 'America/Los_Angeles')
  end

  test 'a New Year\'s Eve sale after midnight in Berlin belongs to the next year' do
    trade(:buy, 1, 10_000, Time.utc(2024, 1, 10), currency: 'EUR')
    trade(:sell, 1, 20_000, Time.utc(2024, 12, 31, 23, 30), currency: 'EUR')

    assert_empty disposals('DE', 2024, time_zone: 'Europe/Berlin')
    assert_equal 1, disposals('DE', 2025, time_zone: 'Europe/Berlin').size
  end

  # --- An in-kind trade fee (Alpaca CFEE) ---

  test 'a coin fee on a buy leaves fewer coins carrying the whole cash paid' do
    alpaca = create(:alpaca_exchange)
    HistoricalPrice.create!(asset: 'BTC', currency: 'USD', date: Date.new(2024, 3, 18), price: 10_000.to_d)
    trade(:buy, 1, 10_000, Time.utc(2024, 3, 18, 15), exchange: alpaca, raw: { 'activity_type' => 'FILL' })
    cfee(alpaca, '0.01', Time.utc(2024, 3, 18))
    trade(:sell, '0.99', 19_800, Time.utc(2024, 6, 1), exchange: alpaca, raw: { 'activity_type' => 'FILL' })

    rows = disposals('US', 2024)

    assert_equal 1, rows.size
    assert_equal [0.99.to_d, 19_800.to_d, 10_000.to_d, 9_800.to_d], rows.first.fields[3..6].map(&:to_d)
  end

  test 'a coin fee on a buy does not take the basis of an older lot' do
    alpaca = create(:alpaca_exchange)
    HistoricalPrice.create!(asset: 'BTC', currency: 'USD', date: Date.new(2024, 3, 18), price: 10_000.to_d)
    trade(:buy, 1, 5_000, Time.utc(2024, 1, 10), exchange: alpaca, raw: { 'activity_type' => 'FILL' })
    trade(:buy, 1, 10_000, Time.utc(2024, 3, 18, 15), exchange: alpaca, raw: { 'activity_type' => 'FILL' })
    cfee(alpaca, '0.01', Time.utc(2024, 3, 18))
    trade(:sell, '1.99', 39_800, Time.utc(2024, 6, 1), exchange: alpaca, raw: { 'activity_type' => 'FILL' })

    rows = disposals('US', 2024)

    assert_equal(15_000.to_d, rows.sum { |row| row[5].to_d })
    assert_equal 1, rows.size
  end

  test 'a coin fee on a sale comes out of the proceeds, not as a disposal of its own' do
    alpaca = create(:alpaca_exchange)
    HistoricalPrice.create!(asset: 'BTC', currency: 'USD', date: Date.new(2024, 6, 1), price: 30_000.to_d)
    trade(:buy, 1, 10_000, Time.utc(2024, 1, 10), exchange: alpaca, raw: { 'activity_type' => 'FILL' })
    trade(:sell, '0.5', 15_000, Time.utc(2024, 6, 1, 15), exchange: alpaca, raw: { 'activity_type' => 'FILL' })
    cfee(alpaca, '0.01', Time.utc(2024, 6, 1))
    trade(:sell, '0.49', 14_700, Time.utc(2024, 7, 1), exchange: alpaca, raw: { 'activity_type' => 'FILL' })

    rows = disposals('US', 2024)

    assert_equal 2, rows.size
    assert_equal [0.51.to_d, 15_000.to_d, 5_100.to_d, 9_900.to_d], rows.first.fields[3..6].map(&:to_d)
    assert_equal [0.49.to_d, 4_900.to_d], [rows.last[3].to_d, rows.last[5].to_d]
  end

  private

  def trade(side, amount, quote, at, currency: 'USD', fee: nil, exchange: @exchange, raw: {})
    AccountTransaction.create!(user: @user, exchange: exchange, entry_type: side, base_currency: 'BTC',
                               base_amount: amount.to_d, quote_currency: currency, quote_amount: quote.to_d,
                               fee_currency: (currency if fee), fee_amount: fee&.to_d,
                               transacted_at: at, raw_data: raw)
  end

  # Alpaca's crypto trading fee: its own row, in the coin, dated to the day (Exchanges::Alpaca#normalize_crypto_fee).
  def cfee(exchange, amount, day)
    AccountTransaction.create!(user: @user, exchange: exchange, entry_type: :fee, base_currency: 'BTC',
                               base_amount: amount.to_d, transacted_at: day,
                               raw_data: { 'activity_type' => 'CFEE', 'date' => day.to_date.iso8601,
                                           'symbol' => 'BTCUSD', 'qty' => "-#{amount}" })
  end

  def disposals(country, year, time_zone: 'UTC')
    csv = Tax::Report.new(country: country, year: year, transactions: AccountTransaction.for_user(@user),
                          time_zone: time_zone).to_csv
    CSV.parse(csv, headers: true).select { |row| row[2] == 'BTC' }
  end
end
