require 'test_helper'

class Tax::EcbFxRatesTest < ActiveSupport::TestCase
  def seed(currency, date, rate)
    FxRate.create!(currency: currency, date: date, rate: rate)
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

  test 'bundesbank_to_sdmx skips the metadata preamble and invalid observations' do
    csv = <<~CSV
      "Name","Daily exchange rates"
      "Source","Deutsche Bundesbank"
      "Last update","2025-03-07"
      "TIME_PERIOD","BBEX3.D.USD.EUR.BB.AC.000","OBS_STATUS"
      "2025-03-03","1.0501",""
      "2025-03-04",".",""
      "2025-03-05","",""
      "2025-03-06","1.0522",""
    CSV

    expected = <<~CSV
      USD,2025-03-03,1.0501
      USD,2025-03-06,1.0522
    CSV

    assert_equal expected, Tax::EcbFxRates.send(:bundesbank_to_sdmx, csv, 'USD')
  end
end
