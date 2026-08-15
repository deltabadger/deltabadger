require 'test_helper'

# Covers the ECB rate reaching a real report, which the Tax::EcbFxRates unit tests cannot:
# an inverted multiplier or a swapped from/to inside PriceService would pass those and fail here.
class Tax::PriceServiceFxTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @api_key = create(:api_key, user: @user)
  end

  # The wealth-snapshot branch of Tax::Report#to_csv never calls prefetch, so this only passes
  # while ensure_loaded! runs from PriceService#initialize. No FxRate row is seeded on purpose —
  # the rate has to arrive through the importer for the holding to be valued at all.
  test 'NL wealth snapshot imports ECB rates and values a stablecoin holding with them' do
    stub_request(:get, /data-api\.ecb\.europa\.eu/).to_return(status: 200, body: <<~CSV)
      CURRENCY,TIME_PERIOD,OBS_VALUE
      USD,2024-12-31,1.25
    CSV
    create(:account_transaction, user: @user, api_key: @api_key, exchange: @api_key.exchange,
                                 entry_type: :buy, base_currency: 'USDT', base_amount: 100,
                                 quote_currency: 'USD', quote_amount: 100,
                                 transacted_at: Time.utc(2024, 12, 15))

    report = Tax::Report.new(country: 'NL', year: 2025, transactions: AccountTransaction.for_user(@user))
    # Headers: reference_date, asset, amount, value, currency
    holding = CSV.parse(report.to_csv).find { |row| row[1] == 'USDT' }

    # Snapshot is 2025-01-01; the 2024-12-31 rate is inside the lookback. 100 USDT * (1 / 1.25) EUR.
    assert_equal 80.to_d, holding[3].to_d
  end

  # Different rates on the two dates: a date mismatch or an inverted multiplier changes both numbers.
  test 'DE report converts USD proceeds and cost basis with the rate of each transaction date' do
    FxRate.create!(currency: 'USD', date: Date.new(2025, 1, 10), rate: '1.25'.to_d)
    FxRate.create!(currency: 'USD', date: Date.new(2025, 6, 10), rate: '2.00'.to_d)
    create(:account_transaction, user: @user, api_key: @api_key, exchange: @api_key.exchange,
                                 entry_type: :buy, base_currency: 'BTC', base_amount: 1,
                                 quote_currency: 'USD', quote_amount: 100,
                                 transacted_at: Time.utc(2025, 1, 10))
    create(:account_transaction, user: @user, api_key: @api_key, exchange: @api_key.exchange,
                                 entry_type: :sell, base_currency: 'BTC', base_amount: 1,
                                 quote_currency: 'USD', quote_amount: 300,
                                 transacted_at: Time.utc(2025, 6, 10))

    report = Tax::Report.new(country: 'DE', year: 2025, transactions: AccountTransaction.for_user(@user))
    # Headers: date, acquisition_date, asset, amount, proceeds, cost_basis, gain_loss, ...
    disposal = CSV.parse(report.to_csv).second

    assert_equal 150.to_d, disposal[4].to_d # 300 USD * (1 / 2.00)
    assert_equal 80.to_d, disposal[5].to_d  # 100 USD * (1 / 1.25)
  end

  test 'cached zero FX fallback marks every transaction sharing the broken pair incomplete' do
    Tax::EcbFxRates.expects(:rate).once.raises(Tax::EcbFxRates::MissingRate)
    timestamp = Time.utc(2025, 1, 10)
    transactions = 2.times.map do
      create(:account_transaction, user: @user, api_key: @api_key, exchange: @api_key.exchange,
                                   base_currency: 'ETH', base_amount: 1,
                                   quote_currency: 'USD', quote_amount: 100,
                                   transacted_at: timestamp)
    end

    enriched = Tax::PriceService.new.enrich(transactions, currency: 'EUR')

    assert_equal [0.to_d, 0.to_d], enriched.pluck(:fiat_value)
    assert_equal [true, true], enriched.pluck(:price_missing)
  end
end
