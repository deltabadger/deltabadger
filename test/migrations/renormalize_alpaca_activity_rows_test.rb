require 'test_helper'
require Rails.root.join('db/migrate/20260815150050_renormalize_alpaca_activity_rows.rb')

# Rows imported by the OLD Alpaca normalizer keep its labels forever: the sync skips a re-fetched
# row whose tx_id is already stored, so nulling `last_synced_at` re-pulls the history and then
# discards every one of these. The reader that arrived with the German broker report then declares
# a dividend as Zeile 19 interest, adds a withholding debit to that income while losing its Zeile 41
# credit, and misses a return of capital's basis reduction entirely.
class RenormalizeAlpacaActivityRowsTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @alpaca = create(:alpaca_exchange)
  end

  test 'a legacy dividend regains the security its Teilfreistellung depends on' do
    row = legacy(entry_type: :other_income, base_currency: 'USD', base_amount: 42, quote_currency: nil,
                 raw: { 'activity_type' => 'DIV', 'symbol' => 'AAPL', 'net_amount' => '42' })

    migrate!

    row.reload
    assert_equal 'other_income', row.entry_type
    assert_equal 'AAPL', row.quote_currency
    assert_equal 'Dividend (AAPL)', row.description
  end

  test 'a legacy withholding debit stops being income and becomes a credit' do
    row = legacy(entry_type: :other_income, base_currency: 'USD', base_amount: 15, quote_currency: nil,
                 raw: { 'activity_type' => 'DIVNRA', 'symbol' => 'AAPL', 'net_amount' => '-15' })
    interest = legacy(entry_type: :other_income, base_currency: 'USD', base_amount: 3, quote_currency: nil,
                      raw: { 'activity_type' => 'INTTW', 'net_amount' => '-3' })

    migrate!

    assert_equal 'withholding_tax', row.reload.entry_type
    assert_equal 'AAPL', row.quote_currency
    assert_equal 'withholding_tax', interest.reload.entry_type
    assert_equal 'Withholding (interest)', interest.description
  end

  test 'a legacy return of capital becomes a basis reduction on its own security' do
    row = legacy(entry_type: :other_income, base_currency: 'USD', base_amount: 12, quote_currency: nil,
                 raw: { 'activity_type' => 'DIVROC', 'symbol' => 'IBIT', 'qty' => '30', 'net_amount' => '12' })

    migrate!

    row.reload
    assert_equal 'return_of_capital', row.entry_type
    assert_equal 'IBIT', row.base_currency
    assert_equal 30.to_d, row.base_amount
    assert_equal 'USD', row.quote_currency
    assert_equal 12.to_d, row.quote_amount
  end

  test 'a legacy crypto fill loses the pair string that hid it from the crypto report' do
    slashed = legacy(entry_type: :buy, base_currency: 'BTC/USD', base_amount: 1, quote_currency: nil,
                     raw: { 'activity_type' => 'FILL', 'symbol' => 'BTC/USD' })
    create(:ticker, exchange: @alpaca, ticker: 'AAVE/USD', base: 'AAVE', quote: 'USD')
    compact = legacy(entry_type: :buy, base_currency: 'AAVEUSD', base_amount: 2, quote_currency: nil,
                     raw: { 'activity_type' => 'FILL', 'symbol' => 'AAVEUSD' })
    stock = legacy(entry_type: :buy, base_currency: 'AAPL', base_amount: 3, quote_currency: 'USD',
                   raw: { 'activity_type' => 'FILL', 'symbol' => 'AAPL' })

    migrate!

    assert_equal %w[BTC USD], [slashed.reload.base_currency, slashed.quote_currency]
    assert_equal %w[AAVE USD], [compact.reload.base_currency, compact.quote_currency]
    assert_equal %w[AAPL USD], [stock.reload.base_currency, stock.quote_currency]
  end

  test 'other venues and already-normalized rows are untouched, and a second run changes nothing' do
    binance = AccountTransaction.create!(user: @user, exchange: create(:binance_exchange),
                                         base_currency: 'BTC', base_amount: 1, entry_type: :buy,
                                         transacted_at: Time.utc(2024, 5, 1),
                                         raw_data: { 'activity_type' => 'DIV', 'symbol' => 'AAPL' })
    fresh = legacy(entry_type: :withholding_tax, base_currency: 'USD', base_amount: 15, quote_currency: 'AAPL',
                   raw: { 'activity_type' => 'DIVNRA', 'symbol' => 'AAPL', 'net_amount' => '-15' })

    migrate!
    migrate!

    assert_equal 'buy', binance.reload.entry_type
    assert_equal 'BTC', binance.base_currency
    assert_equal 'withholding_tax', fresh.reload.entry_type
    assert_equal 'AAPL', fresh.quote_currency
  end

  private

  def legacy(entry_type:, base_currency:, base_amount:, quote_currency:, raw:)
    AccountTransaction.create!(user: @user, exchange: @alpaca, entry_type: entry_type,
                               base_currency: base_currency, base_amount: base_amount,
                               quote_currency: quote_currency, tx_id: raw['activity_type'] + base_currency,
                               transacted_at: Time.utc(2024, 5, 1), raw_data: raw)
  end

  def migrate!
    RenormalizeAlpacaActivityRows.new.tap { |migration| migration.verbose = false }.up
  end
end
