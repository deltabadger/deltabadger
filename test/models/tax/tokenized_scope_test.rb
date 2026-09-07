require 'test_helper'

# A tokenized wrapper (Backed xStock, Ondo, bStocks, PAX Gold) is a claim on an off-chain asset
# through an issuer, not a crypto-native asset. Its German treatment is contested — the leading view
# is §20 EStG with no holding-period exemption, against §23 for crypto — and no authority settles it.
#
# The crypto report must therefore refuse rather than silently apply the one-year exemption. Detection
# has to use the transaction set the CALCULATION consumes: a disposal-only rule is German-shaped and
# misses PVCT, which pools purchases portfolio-wide, and the wealth snapshot, which has no disposals
# at all.
class Tax::TokenizedScopeTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:kraken_exchange)
    @nvdax = create(:asset, symbol: 'NVDAX', name: 'NVIDIA xStock',
                            external_id: 'nvidia-xstock', instrument_type: 'tokenized')
    create(:asset, :bitcoin)
  end

  def tx(currency, at, entry_type: :buy)
    create(:account_transaction, user: @user, exchange: @exchange, base_currency: currency,
                                 entry_type: entry_type, transacted_at: at)
  end

  def report_for(country, year, transactions)
    Tax::Report.new(country: country, year: year, transactions: transactions)
  end

  test 'a tokenized purchase in scope is detected even with no disposal' do
    tx('NVDAX', Time.utc(2024, 3, 1))

    report = report_for('DE', 2024, AccountTransaction.where(user: @user))

    assert_equal ['NVDAX'], report.tokenized_symbols_in_scope
  end

  test 'an ordinary crypto asset is not detected' do
    tx('BTC', Time.utc(2024, 3, 1))

    report = report_for('DE', 2024, AccountTransaction.where(user: @user))

    assert_empty report.tokenized_symbols_in_scope
  end

  test 'activity after the report scope does not count' do
    tx('NVDAX', Time.utc(2026, 3, 1))

    report = report_for('DE', 2024, AccountTransaction.where(user: @user))

    assert_empty report.tokenized_symbols_in_scope, 'a later purchase cannot affect a 2024 report'
  end

  # A prior-year acquisition still sets cost basis for a disposal in scope, so it must be seen.
  test 'an acquisition before the year is in scope' do
    tx('NVDAX', Time.utc(2022, 5, 1))

    report = report_for('DE', 2024, AccountTransaction.where(user: @user))

    assert_equal ['NVDAX'], report.tokenized_symbols_in_scope
  end

  # The Dutch snapshot is taken on 1 January, so a 2 January purchase contributes nothing to it.
  # Report's own year-end scope would wrongly include it.
  test 'a purchase after a wealth snapshot reference date does not count' do
    tx('NVDAX', Time.utc(2024, 1, 2))

    report = report_for('NL', 2024, AccountTransaction.where(user: @user))

    skip 'NL is not a wealth-snapshot jurisdiction here' unless report.send(:wealth_snapshot?)
    assert_empty report.tokenized_symbols_in_scope,
                 'detection must share the calculation cutoff, not the report year'
  end
end
