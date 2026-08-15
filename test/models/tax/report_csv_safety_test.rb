require 'test_helper'

# End-to-end coverage for Tax::Report#to_csv itself, as opposed to the individual
# calculation methods exercised elsewhere in test/models/tax/. Every account
# transaction here quotes directly in the report's own currency, so PriceService
# resolves fiat values from quote_amount without ever calling out for a market price —
# no VCR/webmock scaffolding needed.
class Tax::ReportCsvSafetyTest < ActiveSupport::TestCase
  test 'an asset symbol beginning with a formula leader comes out escaped in a real report' do
    # A current rate row is all it takes to keep PriceService#prefetch off the network:
    # Tax::EcbFxRates.ensure_loaded! sees fresh data and skips the ECB fetch. The rate itself
    # is never read — this report quotes in USD and converts nothing.
    FxRate.create!(currency: 'USD', date: Date.current, rate: '1.05'.to_d)
    user = create(:user)
    api_key = create(:api_key, user: user)

    create(:account_transaction, user: user, api_key: api_key, exchange: api_key.exchange,
                                 entry_type: :buy, base_currency: '=1+1', base_amount: 1,
                                 quote_currency: 'USD', quote_amount: 100,
                                 transacted_at: Time.utc(2026, 1, 1))
    create(:account_transaction, user: user, api_key: api_key, exchange: api_key.exchange,
                                 entry_type: :sell, base_currency: '=1+1', base_amount: 1,
                                 quote_currency: 'USD', quote_amount: 150,
                                 transacted_at: Time.utc(2026, 6, 1))

    report = Tax::Report.new(country: 'US', year: 2026, transactions: AccountTransaction.for_user(user))
    csv = report.to_csv

    refute_includes csv, ',=1+1'
    assert_includes csv, ",'=1+1"
  end
end
