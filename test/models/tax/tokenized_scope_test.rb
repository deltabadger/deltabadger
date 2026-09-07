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
  # Already-synced Kraken history stores the venue's own code; the importer's normalisation only
  # helps rows inserted after it, and a re-sync skips ids it already has.
  test 'legacy venue codes already in the ledger are still detected' do
    ticker_for_nvdax
    # One api_key: its factory builds its own exchange, so creating two would collide on the
    # single-row-per-type constraint.
    key = create(:api_key, user: @user, exchange: @exchange)
    %w[NVDAx NVDASPV].each_with_index do |code, i|
      create(:account_transaction, user: @user, exchange: @exchange, api_key: key, base_currency: code,
                                   entry_type: :buy, transacted_at: Time.utc(2024, 3, 1 + i))
    end

    report = report_for('DE', 2024, AccountTransaction.where(user: @user))

    assert_equal ['NVDAX'], report.tokenized_symbols_in_scope.uniq,
                 'a stored NVDAx or NVDASPV must still refuse the report'
  end

  # The wealth engine excludes transactions AT the cutoff instant, so detection must too.
  test 'a purchase exactly at a wealth snapshot cutoff does not block' do
    ticker_for_nvdax
    create(:account_transaction, user: @user, exchange: @exchange, base_currency: 'NVDAX',
                                 entry_type: :buy, transacted_at: Time.utc(2024, 1, 1))

    report = report_for('NL', 2024, AccountTransaction.where(user: @user))

    skip 'NL is not a wealth-snapshot jurisdiction here' unless report.send(:wealth_snapshot?)
    assert_empty report.tokenized_symbols_in_scope
  end

  def ticker_for_nvdax
    usd = create(:asset, :usd)
    create(:ticker, exchange: @exchange, base_asset: @nvdax, quote_asset: usd,
                    base: 'NVDAX', quote: 'USD', ticker: 'NVDAxUSD', trading_enabled: false)
  end
  # A wrapper the catalogue files under its own category — Hyperliquid's RWA shim assets are
  # "Tokenized Stock" — is rejected by AssetIdentity on a crypto venue, so the identity path alone
  # would let it through. The venue's own listing answers "is this a wrapper" without that filter.
  test 'a wrapper listed under a non-crypto category is still detected' do
    rwa = create(:asset, symbol: 'TSLAH', name: 'Tesla (Hyperliquid)', external_id: 'tesla-hl',
                         category: 'Tokenized Stock', instrument_type: 'tokenized')
    usd = create(:asset, :usd)
    create(:ticker, exchange: @exchange, base_asset: rwa, quote_asset: usd,
                    base: 'TSLAH', quote: 'USD', ticker: 'TSLAHUSD', trading_enabled: false)
    create(:account_transaction, user: @user, exchange: @exchange, base_currency: 'TSLAH',
                                 entry_type: :buy, transacted_at: Time.utc(2024, 3, 1))

    report = report_for('DE', 2024, AccountTransaction.where(user: @user))

    assert_equal ['TSLAH'], report.tokenized_symbols_in_scope
  end
end
