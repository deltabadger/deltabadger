require 'test_helper'

# Where a legless row gets its value.
#
# Kraken books every trade as one row per asset with no quote; Binance books every Convert and dust
# sweep as swap legs with no quote. Today the coin leg is priced off a chart while the cash leg beside
# it says exactly what was paid or received. The cash leg IS the quote: consulted right after a
# stated value and a quote of the row's own, ahead of any price lookup — so a cash-backed leg is
# never fetched, never warns, and never overwrites what the user stated.
class Tax::PriceServiceSwapLegsTest < ActiveSupport::TestCase
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    MarketData.stubs(:get_historical_price_range).returns(Result::Failure.new('offline'))
    @user = create(:user)
    @kraken = create(:kraken_exchange)
    @binance = create(:binance_exchange)
    @key_kraken = create(:api_key, user: @user, exchange: @kraken)
    @key_binance = create(:api_key, user: @user, exchange: @binance)
    @at = Time.utc(2026, 1, 5, 12)
    FxRate.create!(currency: 'USD', date: @at.to_date, rate: 1.25) # 1 EUR = 1.25 USD
  end

  def leg(type, asset, amount, key: @key_kraken, group: 'ref-1', **attrs)
    create(:account_transaction, api_key: key, exchange: key.exchange, entry_type: type, base_currency: asset,
                                 base_amount: amount, quote_currency: nil, quote_amount: nil, group_id: group,
                                 transacted_at: @at, **attrs)
  end

  def price(symbol, usd)
    HistoricalPrice.create!(asset: symbol, currency: 'USD', date: @at.to_date, price: usd)
  end

  # Rows keyed by asset — every test holds one row per asset.
  def enrich(currency: 'USD')
    service = Tax::PriceService.new
    rows = service.enrich(AccountTransaction.for_user(@user).order(:transacted_at, :id).to_a, currency: currency)
    [rows.index_by { |row| row[:base_currency] }, service]
  end

  test 'a coin bought against a cash row costs what the cash row says' do
    leg(:sell, 'EUR', 1_000)
    leg(:buy, 'BTC', 0.05)

    rows, service = enrich

    assert_equal 1_250.to_d, rows['BTC'][:fiat_value]
    assert_not rows['BTC'][:price_missing]
    assert_empty service.warnings, 'no chart was consulted'
  end

  test 'a coin sold for a cash row brings in what the cash row says' do
    leg(:sell, 'BTC', 0.05)
    leg(:buy, 'EUR', 1_000)

    rows, = enrich

    assert_equal 1_250.to_d, rows['BTC'][:fiat_value]
    assert_equal 'EUR', rows['BTC'][:quote_currency], 'so every engine sees a sale for cash'
    assert_not rows['BTC'][:price_missing]
  end

  test 'a convert into a stablecoin is a sale' do
    leg(:swap_out, 'BNB', 1, key: @key_binance, group: 'convert-1')
    leg(:swap_in, 'USDT', 300, key: @key_binance, group: 'convert-1')

    rows, = enrich

    assert_equal 300.to_d, rows['BNB'][:fiat_value]
    assert_equal 'USDT', rows['BNB'][:quote_currency]
    assert_not rows['BNB'][:price_missing]
  end

  test 'a convert from a stablecoin states the cash spent, never a market guess' do
    leg(:swap_out, 'USDT', 150, key: @key_binance, group: 'convert-2')
    leg(:swap_in, 'LTC', 0.9, key: @key_binance, group: 'convert-2')

    rows, service = enrich

    assert_equal 150.to_d, rows['LTC'][:fiat_value]
    assert_equal 150.to_d, rows['LTC'][:swap_stable_cost]
    assert_empty service.warnings
  end

  test 'a mixed sweep keeps the market value and carries the cash beside it' do
    price('ETH', 3_000)
    price('BNB', 400)
    leg(:swap_out, 'ETH', 0.01, key: @key_binance, group: 'dust-1')
    leg(:swap_out, 'USDT', 5, key: @key_binance, group: 'dust-1')
    leg(:swap_in, 'BNB', 0.05, key: @key_binance, group: 'dust-1')

    rows, = enrich

    assert_equal 20.to_d, rows['BNB'][:fiat_value], 'what the BNB was worth — the taxable engines open the lot at it'
    assert_equal 5.to_d, rows['BNB'][:swap_stable_cost], 'what the cash leg paid — the chain adds it to the ETH basis'
  end

  test 'in a euro report the cash leg is stated in euro' do
    leg(:sell, 'EUR', 1_000)
    leg(:buy, 'BTC', 0.05)

    rows, = enrich(currency: 'EUR')

    assert_equal 1_000.to_d, rows['BTC'][:fiat_value]
  end

  # A cash figure in a currency with no rate is no valuation at all, so the coin is priced as any
  # other row would be — and says so on its own terms when that fails too.
  test 'a cash row that cannot be converted leaves the coin to the ordinary lookup' do
    leg(:sell, 'AED', 100)
    leg(:buy, 'BTC', 0.01)

    rows, service = enrich

    assert rows['BTC'][:price_missing]
    assert(service.warnings.any? { |warning| warning.include?('BTC') })
    assert(service.warnings.none? { |warning| warning.include?('AED') }, 'the cash leg itself is not a hole')
  end

  test 'a coin traded for a coin is a swap, however the venue books it' do
    price('BTC', 20_000)
    price('ETH', 2_000)
    leg(:sell, 'BTC', 1)
    leg(:buy, 'ETH', 10)

    rows, = enrich

    assert_equal 'swap_out', rows['BTC'][:entry_type].to_s
    assert_equal 'swap_in', rows['ETH'][:entry_type].to_s
  end

  test 'a fee never crosses exchanges' do
    price('ETH', 2_000)
    leg(:sell, 'EUR', 1_000, fee_currency: 'EUR', fee_amount: 5)
    leg(:buy, 'BTC', 0.05)
    leg(:buy, 'ETH', 1, key: @key_binance) # same group id, another venue

    rows, = enrich

    assert_equal 5.to_d, rows['BTC'][:fee_amount], 'the EUR row\'s fee belongs to the BTC it bought'
    assert_nil rows['ETH'][:fee_amount]
  end

  # The cash row IS the venue's figure: a typed value is neither written in front of it nor read
  # if one was written before the rule.
  test 'the cash row stands in front of a stated value' do
    leg(:sell, 'EUR', 1_000)
    bought = leg(:buy, 'BTC', 0.05)
    assert_raises(ArgumentError) { bought.set_manual(:price, 999) }
    bought.update_column(:manual_values, { 'price' => '999' })

    rows, = enrich

    assert_equal 1_250.to_d, rows['BTC'][:fiat_value]
    assert_not rows['BTC'][:stated_value]
  end

  # ── the order the ledger is walked in ────────────────────────────────────────────────────
  test 'an out-leg goes before its own in-leg at the same instant; an unrelated sale keeps its place' do
    price('BTC', 20_000)
    price('ETH', 2_000)
    bought = leg(:buy, 'BTC', 1, group: nil)
    sold = leg(:sell, 'BTC', 1, group: nil, quote_currency: 'USDT', quote_amount: 20_000)
    swapped_in = leg(:swap_in, 'ETH', 10, key: @key_binance, group: 'convert-9')
    swapped_out = leg(:swap_out, 'BTC', 1, key: @key_binance, group: 'convert-9')

    ordered = Tax::PriceService.ordered(AccountTransaction.for_user(@user).to_a)

    assert_equal [bought, sold, swapped_out, swapped_in].map(&:id), ordered.map(&:id)
  end

  # ── rows that leave without a sale ───────────────────────────────────────────────────────
  #
  # A withdrawal or a lost coin is valued for the record like any row, but no engine reads that
  # value, so a chart with no price for it is not a hole in the report: the base lookup never warns.
  # A fee on such a row does reach the figures, and warns as on any row.
  test 'a withdrawal is valued for the record and never warns for it' do
    price('BTC', 20_000)
    leg(:withdrawal, 'BTC', 1, group: nil)
    leg(:withdrawal, 'ETH', 1, group: nil) # no price anywhere

    rows, service = enrich

    assert_equal 20_000.to_d, rows['BTC'][:fiat_value]
    assert_equal 0.to_d, rows['ETH'][:fiat_value]
    assert_not rows['ETH'][:price_missing]
    assert_empty service.warnings
  end

  test 'a fee on a withdrawal still warns' do
    price('BTC', 20_000)
    leg(:withdrawal, 'BTC', 1, group: nil, fee_currency: 'ETH', fee_amount: 0.01)

    rows, service = enrich

    assert rows['BTC'][:price_missing]
    assert(service.warnings.any? { |warning| warning.include?('ETH') })
  end

  test 'a lost coin is valued for the record and never warns for it' do
    leg(:lost, 'ETH', 1, group: nil)

    rows, service = enrich

    assert_equal 0.to_d, rows['ETH'][:fiat_value]
    assert_not rows['ETH'][:price_missing]
    assert_empty service.warnings
  end
end
