require 'test_helper'

# BitMart shut down and honeymaker 0.10.0 dropped the venue, so there is no client to talk to.
# Exchanges::Bitmart survives ONLY so an install still holding Bitmart bots and trade history keeps
# loading through STI and can move those bots to a live exchange. These tests pin the two halves of
# that contract: the venue is never tradeable, and nothing it touches raises — every caller has to
# land on an ordinary failure branch, because a raise would break the very pages and jobs that keep
# the orphaned bots reachable.
class RetiredExchangeTest < ActiveSupport::TestCase
  setup do
    @retired = Exchanges::Bitmart.new(name: 'Bitmart', available: false)
  end

  test 'a live exchange is not retired' do
    refute_predicate Exchanges::Binance.new(name: 'Binance'), :retired?
  end

  test 'Bitmart is retired' do
    assert_predicate @retired, :retired?
  end

  test 'the tradeable scope excludes a retired exchange even when it is still marked available' do
    live = create(:binance_exchange, available: true)
    # available: true is deliberately wrong here — `tradeable` must not depend on the flag alone.
    retired = Exchanges::Bitmart.create!(name: 'Bitmart', available: true)

    assert_includes Exchange.tradeable, live
    refute_includes Exchange.tradeable, retired
  end

  # Every method that would have reached the exchange API. A raise here is the failure mode this
  # design exists to prevent: Measurable#metrics_with_current_prices branches on result.failure?,
  # and the order jobs classify result.errors — both would blow up on NotImplementedError.
  test 'read and trade calls return a failure Result instead of raising' do
    ticker = Ticker.new(ticker: 'BTCUSDT')

    results = {
      get_tickers_info: -> { @retired.get_tickers_info },
      get_tickers_prices: -> { @retired.get_tickers_prices },
      get_balances: -> { @retired.get_balances },
      get_last_price: -> { @retired.get_last_price(ticker: ticker) },
      get_bid_price: -> { @retired.get_bid_price(ticker: ticker) },
      get_ask_price: -> { @retired.get_ask_price(ticker: ticker) },
      get_candles: -> { @retired.get_candles(ticker: ticker, start_at: 1.day.ago, timeframe: 1.day) },
      get_order: -> { @retired.get_order(order_id: 'x') },
      get_orders: -> { @retired.get_orders(order_ids: %w[x]) },
      cancel_order: -> { @retired.cancel_order(order_id: 'x') },
      get_api_key_validity: -> { @retired.get_api_key_validity(api_key: nil) },
      market_buy: -> { @retired.market_buy(ticker: ticker, amount: 1, amount_type: :quote) },
      market_sell: -> { @retired.market_sell(ticker: ticker, amount: 1, amount_type: :base) },
      limit_buy: -> { @retired.limit_buy(ticker: ticker, amount: 1, amount_type: :quote, price: 1) },
      limit_sell: -> { @retired.limit_sell(ticker: ticker, amount: 1, amount_type: :base, price: 1) },
      withdraw: -> { @retired.withdraw(asset: nil, amount: 1, address: 'addr') },
      fetch_withdrawal_fees!: -> { @retired.fetch_withdrawal_fees! },
      get_ledger: -> { @retired.get_ledger(api_key: nil) }
    }

    results.each do |name, call|
      result = call.call
      assert_predicate result, :failure?, "#{name} should return a failure Result"
      assert result.errors.first.present?, "#{name} should explain why it failed"
    end
  end

  # Two callers expect a plain value, not a Result — handing them a Result::Failure would be a
  # silently truthy object.
  test 'non-Result contracts keep their real shapes' do
    assert_nil @retired.list_withdrawal_addresses(asset: nil)
    assert_kind_of Symbol, @retired.minimum_amount_logic
  end

  # Automation::ExchangeConnectable#ensure_exchange_authenticated calls both on every bot action,
  # and the base class defines neither.
  test 'set_client and api_key stay available so ensure_exchange_authenticated works' do
    api_key = create(:api_key, exchange: create(:binance_exchange))

    assert_nil @retired.api_key
    @retired.set_client(api_key: api_key)
    assert_equal api_key, @retired.api_key
  end

  # Exchange#symbols builds an ExchangeMarket, whose initializer calls Honeymaker.exchange(name_id)
  # — and honeymaker 0.10.0 has no 'bitmart'.
  test 'symbols never reaches honeymaker' do
    ExchangeMarket.expects(:new).never

    result = @retired.symbols

    assert_predicate result, :success?
    assert_empty result.data
  end

  test 'known_errors is a hash so the shared error classifiers keep working' do
    assert_kind_of Hash, @retired.known_errors
    refute @retired.throttled_error?(['some error'])
    # NETWORK_TRANSIENT_PATTERNS must still apply even with no exchange-specific patterns.
    assert @retired.transient_error?(['Net::OpenTimeout'])
  end
end
