require 'test_helper'

class Tax::EcbFxRatesTest < ActiveSupport::TestCase
  def seed(currency, date, rate)
    FxRate.create!(currency: currency, date: date, rate: rate)
  end

  def sdmx(currency, date, rate)
    "CURRENCY,TIME_PERIOD,OBS_VALUE\n#{currency},#{date},#{rate}\n"
  end

  test 'rate is a multiplier: amount_in_to = amount_in_from * rate' do
    seed('USD', Date.new(2025, 3, 3), '1.10'.to_d)
    seed('GBP', Date.new(2025, 3, 3), '0.85'.to_d)
    # 110 USD -> EUR: 110 * (1/1.10) = 100 EUR
    assert_equal 100.to_d, (110.to_d * Tax::EcbFxRates.rate(from: 'USD', to: 'EUR', date: Date.new(2025, 3, 3))).round(6)
    # USD -> GBP multiplier = 0.85 / 1.10
    assert_equal ('0.85'.to_d / '1.10'.to_d), Tax::EcbFxRates.rate(from: 'USD', to: 'GBP', date: Date.new(2025, 3, 3))
  end

  test 'weekend date falls back to previous published day' do
    seed('USD', Date.new(2025, 2, 28), '1.04'.to_d) # Friday
    assert_equal (1.to_d / '1.04'.to_d), Tax::EcbFxRates.rate(from: 'USD', to: 'EUR', date: Date.new(2025, 3, 2)) # Sunday
  end

  test 'covers every registry report currency' do
    currencies = Tax::Jurisdictions::REGISTRY.values.flat_map { |j| [j[:currency], *j[:currency_by_year]&.values] }.compact.uniq - ['EUR']
    assert_empty currencies - Tax::EcbFxRates::CURRENCIES, 'registry currency missing from ECB importer list'
  end

  test 'raises MissingRate beyond 7-day lookback' do
    assert_raises(Tax::EcbFxRates::MissingRate) do
      Tax::EcbFxRates.rate(from: 'USD', to: 'EUR', date: Date.new(2025, 3, 2))
    end
  end

  test 'identity' do
    assert_equal 1.to_d, Tax::EcbFxRates.rate(from: 'EUR', to: 'EUR', date: Date.new(2025, 3, 3))
  end

  test 'ensure_loaded! parses ECB SDMX csvdata and is idempotent' do
    # SDMX csvdata: one row per (series, date); CURRENCY and TIME_PERIOD/OBS_VALUE columns.
    csv = <<~CSV
      KEY,FREQ,CURRENCY,CURRENCY_DENOM,EXR_TYPE,EXR_SUFFIX,TIME_PERIOD,OBS_VALUE
      EXR.D.USD.EUR.SP00.A,D,USD,EUR,SP00,A,2025-03-03,1.0501
      EXR.D.USD.EUR.SP00.A,D,USD,EUR,SP00,A,2025-02-28,1.0384
      EXR.D.GBP.EUR.SP00.A,D,GBP,EUR,SP00,A,2025-03-03,0.8262
      EXR.D.GBP.EUR.SP00.A,D,GBP,EUR,SP00,A,2025-02-28,0.8255
    CSV
    Tax::EcbFxRates.stubs(:fetch_history_csv).returns(csv)
    Tax::EcbFxRates.ensure_loaded!
    Tax::EcbFxRates.ensure_loaded! # second call: no dupes
    assert_equal '1.0501'.to_d, FxRate.find_by(currency: 'USD', date: Date.new(2025, 3, 3)).rate
    assert_equal 4, FxRate.count
  end

  # Verbatim from the live feed: a UTF-8 BOM immediately in front of an empty quoted field,
  # a half-quoted metadata preamble, "." for a day with no publication and a bare comma for
  # a missing one. CSV.parse on the raw body reads the BOM bytes as field content and then
  # hits a quote mid-field, which is how every fall back to this source used to die.
  test 'bundesbank_to_sdmx keeps only real observations from the live feed shape' do
    csv = "\uFEFF#{<<~CSV}"
      "",BBEX3.D.USD.EUR.BB.AC.000,BBEX3.D.USD.EUR.BB.AC.000_FLAGS
      "",Euro foreign exchange reference rate of the ECB / EUR 1 = USD / United States,
      Comment (in english),"The ECB publishes daily euro foreign exchange reference rates, which are calculated on the basis of the concertation between central banks at 14.15.",
      Decimals,4,
      last update,2026-09-07 15:58:27,
      1999-01-01,.,No value available
      1999-01-02,,
      1999-01-04,1.1789,
      2026-09-04,1.1712,
    CSV

    expected = <<~CSV
      USD,1999-01-04,1.1789
      USD,2026-09-04,1.1712
    CSV

    assert_equal expected, Tax::EcbFxRates.send(:bundesbank_to_sdmx, csv, 'USD')
  end

  # The ECB only publishes on business days, so from Saturday until Monday's publication
  # the newest stored rate is Friday's. Comparing against a plain "yesterday" made that
  # look stale and refetched the whole 1999-onwards history on every single job run.
  test 'no refetch over the weekend: Friday satisfies Saturday through Monday' do
    seed('USD', Date.new(2026, 9, 4), '1.1712'.to_d) # Friday

    Tax::EcbFxRates.stubs(:fetch_history_csv).returns(sdmx('USD', '2026-09-07', '9.9999'))
    [Date.new(2026, 9, 5), Date.new(2026, 9, 6), Date.new(2026, 9, 7)].each do |today|
      travel_to(today) { Tax::EcbFxRates.ensure_loaded! }
    end

    assert_equal 1, FxRate.count, 'refetched the full history while the ECB had published nothing new'
  end

  test 'refetches on Tuesday once Monday publication is due' do
    seed('USD', Date.new(2026, 9, 4), '1.1712'.to_d) # Friday

    Tax::EcbFxRates.stubs(:fetch_history_csv).returns(sdmx('USD', '2026-09-07', '1.1690'))
    travel_to(Date.new(2026, 9, 8)) { Tax::EcbFxRates.ensure_loaded! }

    assert_equal '1.1690'.to_d, FxRate.find_by(currency: 'USD', date: Date.new(2026, 9, 7))&.rate
  end
end
