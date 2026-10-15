require 'test_helper'

# A broker can list a security and a coin under one ticker (Alpaca's BTC beside BTC/USD), and prices
# are stored by symbol. A coin's prices live under the bare symbol; a security's go under `stock:SYM`
# whenever a coin has that symbol too, so neither can ever be read as the other.
class Tax::PriceServiceSecurityTest < ActiveSupport::TestCase
  DAY = Time.utc(2026, 10, 8, 12)

  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    @user = create(:user)
    @alpaca = create(:alpaca_exchange)
    @alpaca_key = create(:api_key, user: @user, exchange: @alpaca)
    @usd = Asset.find_by(symbol: 'USD') || create(:asset, symbol: 'USD', external_id: 'usd', name: 'US Dollar', category: 'Fiat')
    @fetched = []
    @answers = {}
  end

  def ms(date) = date.to_time(:utc).to_i * 1000

  def answer(coin_id, prices) = @answers[coin_id] = prices.map { |date, price| [ms(date), price] }

  # Every request data-api is asked, as [coin, currency, first day, last day].
  def market(**args)
    @fetched << [args[:coin_id], args[:currency], args[:from].to_date, args[:to].to_date - 1]
    Result::Success.new('prices' => @answers.fetch(args[:coin_id], []))
  end

  def bitcoin = Asset.find_by(external_id: 'bitcoin') || create(:asset, :bitcoin)

  def security(symbol, external_id) = create(:asset, symbol: symbol, external_id: external_id, name: symbol, category: 'Stock')

  def list(exchange, asset, ticker)
    create(:ticker, exchange: exchange, base_asset: asset, quote_asset: @usd, ticker: ticker, base: asset.symbol, quote: 'USD')
  end

  # A quoteless row: what Alpaca books a crypto fee (CFEE) as.
  def fee(symbol, at: DAY, base_asset: nil, key: @alpaca_key)
    create(:account_transaction, api_key: key, entry_type: :fee, base_currency: symbol, base_asset: base_asset,
                                 base_amount: '0.001', quote_currency: nil, quote_amount: nil, transacted_at: at)
  end

  def enrich
    test = self
    MarketData.singleton_class.alias_method(:__range, :get_historical_price_range)
    MarketData.define_singleton_method(:get_historical_price_range) { |**args| test.market(**args) }
    Tax::PriceService.new.enrich(Tax::PriceService.ordered(AccountTransaction.for_user(@user).to_a), currency: 'USD')
  ensure
    MarketData.singleton_class.alias_method(:get_historical_price_range, :__range)
  end

  def alpaca_btc
    @coin = bitcoin
    @security = security('BTC', 'BTC.US')
    list(@alpaca, @security, 'BTC')
    list(@alpaca, @coin, 'BTC/USD')
  end

  test 'a security no coin shares a symbol with is fetched and stored exactly as before' do
    list(@alpaca, security('AAPL', 'AAPL.US'), 'AAPL')
    answer('AAPL.US', [[Date.new(2026, 9, 1), 230.5]])
    fee('AAPL', at: Time.utc(2026, 9, 1, 12))

    rows = travel_to(Time.utc(2026, 10, 15, 12)) { enrich }

    assert_equal [['AAPL.US', 'usd', Date.new(2026, 9, 1), Date.new(2026, 9, 1)]], @fetched
    assert_equal [['AAPL', 'USD', Date.new(2026, 9, 1), 230.5.to_d]],
                 HistoricalPrice.pluck(:asset, :currency, :date, :price)
    assert_equal 0.2305.to_d, rows.sole[:fiat_value]
  end

  test 'a coin fee at a broker that lists a same-ticker security is priced as the coin it recorded' do
    alpaca_btc
    answer('bitcoin', [[DAY.to_date, 83_000]])
    fee('BTC', base_asset: @coin)

    rows = travel_to(Time.utc(2026, 10, 15, 12)) { enrich }

    assert_equal %w[bitcoin], @fetched.map(&:first).uniq, 'never the security'
    assert_equal({ 'BTC' => 83_000.to_d }, HistoricalPrice.pluck(:asset, :price).to_h)
    assert_equal 83.to_d, rows.sole[:fiat_value]
  end

  test 'a coin fee that recorded no asset there is valued as the coin the venue\'s other rows recorded' do
    alpaca_btc
    answer('bitcoin', [[DAY.to_date, 83_000]])
    create(:account_transaction, api_key: @alpaca_key, entry_type: :buy, base_currency: 'BTC', base_asset: @coin,
                                 base_amount: 1, quote_currency: 'USD', quote_amount: 80_000, transacted_at: DAY - 1.day)
    fee('BTC')

    rows = travel_to(Time.utc(2026, 10, 15, 12)) { enrich }

    assert_equal 83.to_d, rows.last[:fiat_value]
    assert_equal %w[bitcoin], @fetched.map(&:first).uniq
  end

  test 'a name that is still both a security and a coin is left unpriced, and nothing is fetched' do
    alpaca_btc
    fee('BTC')

    rows = travel_to(Time.utc(2026, 10, 15, 12)) { enrich }

    assert rows.sole[:price_missing]
    assert_empty @fetched
    assert_empty HistoricalPrice.all
  end

  test 'a security that shares its symbol with a coin is stored under its own key' do
    alpaca_btc
    answer('BTC.US', [[DAY.to_date, 36.2]])
    HistoricalPrice.create!(asset: 'BTC', currency: 'USD', date: DAY.to_date, price: 83_000)
    fee('BTC', base_asset: @security)

    rows = travel_to(Time.utc(2026, 10, 15, 12)) { enrich }

    assert_equal 0.0362.to_d, rows.sole[:fiat_value], 'the security\'s close, never the coin\'s'
    assert_equal({ 'BTC' => 83_000.to_d, 'stock:BTC' => 36.2.to_d }, HistoricalPrice.pluck(:asset, :price).to_h)
  end

  # The key is shared by every venue: a broker listing only the security must not write the bare key
  # another venue's bitcoin reads, whichever of the two is priced first.
  test 'a broker listing only the security never shares a key with bitcoin held elsewhere' do
    coin = bitcoin
    list(@alpaca, security('BTC', 'BTC.US'), 'BTC')
    binance = create(:binance_exchange)
    binance_key = create(:api_key, user: @user, exchange: binance)
    answer('BTC.US', [[DAY.to_date, 36.2]])
    answer('bitcoin', [[DAY.to_date, 83_000]])
    [[DAY, DAY + 1.hour], [DAY + 1.hour, DAY]].each do |alpaca_at, binance_at|
      AccountTransaction.delete_all
      HistoricalPrice.delete_all
      fee('BTC', at: alpaca_at)
      fee('BTC', at: binance_at, base_asset: coin, key: binance_key)

      rows = travel_to(Time.utc(2026, 10, 15, 12)) { enrich }

      values = rows.to_h { |row| [row[:exchange], row[:fiat_value]] }
      assert_equal({ 'alpaca' => 0.0362.to_d, 'binance' => 83.to_d }, values)
      assert_equal({ 'BTC' => 83_000.to_d, 'stock:BTC' => 36.2.to_d }, HistoricalPrice.pluck(:asset, :price).to_h)
    end
  end

  # A coin a stock venue's rows recorded keeps the dated cut where its symbol changed coin.
  test 'a recorded coin at a broker is still fetched as the coin of each day' do
    old = create(:asset, symbol: 'LUNA', name: 'Terra Classic', external_id: 'terra-luna', category: 'Cryptocurrency')
    create(:asset, symbol: 'LUNA', name: 'Terra', external_id: 'terra-luna-2', category: 'Cryptocurrency', market_cap_rank: 300)
    fee('LUNA', at: Time.utc(2022, 5, 20, 12), base_asset: old)
    fee('LUNA', at: Time.utc(2022, 6, 5, 12), base_asset: old)

    travel_to(Time.utc(2026, 10, 15, 12)) { enrich }

    assert_equal [['terra-luna', Date.new(2022, 5, 20), Date.new(2022, 5, 20)],
                  ['terra-luna-2', Date.new(2022, 6, 5), Date.new(2022, 6, 5)]],
                 @fetched.map { |coin, _, from, to| [coin, from, to] }.first(2)
    singles = @fetched.drop(2).map { |coin, _, from, to| [coin, from..to] }
    assert singles.all? { |coin, days| coin == 'terra-luna' ? days.end <= Date.new(2022, 5, 27) : days.begin >= Date.new(2022, 5, 28) },
           "a one-day window never crosses the cut: #{singles.inspect}"
  end

  # A coin recorded under the name in a row a calculation leaves out (a margin row, say) still owns the
  # bare key: the security beside it is stored under its own, as the transactions table reads it.
  test 'a coin recorded in a row the calculation leaves out still keeps the security off its key' do
    list(@alpaca, security('ABC', 'ABC.US'), 'ABC')
    renamed = create(:asset, symbol: 'ABCX', external_id: 'abc-coin', name: 'ABC coin', category: 'Cryptocurrency')
    answer('ABC.US', [[DAY.to_date, 12]])
    fee('ABC')
    create(:account_transaction, api_key: create(:api_key, user: @user, exchange: create(:binance_exchange)), entry_type: :fee,
                                 base_currency: 'ABC', base_asset: renamed, base_amount: 1, quote_currency: nil, quote_amount: nil,
                                 transacted_at: DAY, tx_id: 'margin-interest-1-ABC')
    walked = AccountTransaction.for_user(@user).to_a.reject { |tx| Tracker::UnfundedCash.borrowed?(tx.tx_id) }

    test = self
    MarketData.singleton_class.alias_method(:__range, :get_historical_price_range)
    MarketData.define_singleton_method(:get_historical_price_range) { |**args| test.market(**args) }
    travel_to(Time.utc(2026, 10, 15, 12)) { Tax::PriceService.new.enrich(walked, currency: 'USD') }
    MarketData.singleton_class.alias_method(:get_historical_price_range, :__range)

    assert_equal({ 'stock:ABC' => 12.to_d }, HistoricalPrice.pluck(:asset, :price).to_h)
  end
end
