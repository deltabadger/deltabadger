require 'test_helper'

class Exchanges::BitrueTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:bitrue_exchange)
  end

  test 'coingecko_id returns bitrue' do
    assert_equal 'bitrue', @exchange.coingecko_id
  end

  test 'known_errors returns expected error messages' do
    errors = @exchange.known_errors
    assert errors[:insufficient_funds].present?
    assert errors[:invalid_key].present?
    assert_includes errors[:insufficient_funds], 'Insufficient balance.'
    assert_includes errors[:invalid_key], 'Invalid Api-Key ID.'
  end

  test 'minimum_amount_logic returns base_and_quote for market orders' do
    assert_equal :base_and_quote, @exchange.minimum_amount_logic(order_type: :market_order)
  end

  test 'minimum_amount_logic returns base_and_quote_in_base for limit orders' do
    assert_equal :base_and_quote_in_base, @exchange.minimum_amount_logic(order_type: :limit_order)
  end

  test 'set_client creates a Honeymaker Bitrue client' do
    @exchange.set_client
    assert_kind_of Honeymaker::Clients::Bitrue, @exchange.send(:client)
  end

  test 'set_client with api_key stores the api_key' do
    api_key = create(:api_key, exchange: @exchange)
    @exchange.set_client(api_key: api_key)
    assert_equal api_key, @exchange.api_key
  end

  test 'requires_passphrase? returns false' do
    assert_equal false, @exchange.requires_passphrase?
  end

  test 'get_api_key_validity checks canTrade for trading keys' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Bitrue.any_instance.stubs(:account_information).returns(
      Result::Success.new({ 'canTrade' => true, 'canWithdraw' => false, 'balances' => [] })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity rejects trading key without canTrade' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Bitrue.any_instance.stubs(:account_information).returns(
      Result::Success.new({ 'canTrade' => false, 'canWithdraw' => true, 'balances' => [] })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  test 'get_api_key_validity accepts withdrawal key with successful account_information' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Bitrue.any_instance.stubs(:account_information).returns(
      Result::Success.new({ 'canTrade' => false, 'canWithdraw' => true, 'balances' => [] })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  # ---- get_candles (Bitrue /api/v1/market/kline) ----
  # Bitrue returns {"symbol","scale","data"=>[{is,o,h,l,c,v}]} (Hash + object candles); the app used to
  # parse only a bare Array -> []. It serves a recent window and ignores startTime. '3d' is unsupported and
  # '1M' means one MINUTE on Bitrue, so 3d/1M are built from the daily resolution.

  test 'get_candles parses the Bitrue hash response, trimmed and sorted' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT')
    now = Time.now.utc
    data = (0...500).map do |i|
      t = (now - i.days).to_i * 1000
      { 'i' => t / 1000, 'is' => t, 'o' => '100', 'h' => '120', 'l' => '90', 'c' => '110', 'v' => '5' }
    end
    seen = {}
    client = Object.new
    client.define_singleton_method(:candlestick_data) do |**kw|
      seen[:interval] = kw[:interval]
      Result::Success.new({ 'symbol' => 'BTCUSDT', 'scale' => 'KLINE_1DAY', 'data' => data })
    end
    @exchange.stubs(:client).returns(client)

    deep = @exchange.get_candles(ticker: ticker, start_at: 20.years.ago, timeframe: 1.day)
    assert_predicate deep, :success?
    assert_equal 500, deep.data.size, 'must parse the hash data array (was [] before)'
    assert_equal '1d', seen[:interval]
    ts = deep.data.map { |c| c[0] }
    assert_equal ts.sort, ts, 'sorted ascending'
    assert_operator deep.data.last[0], :>=, now - 2.days
    assert_equal 100.to_d, deep.data.last[1], 'open mapped from o'
    assert_equal 110.to_d, deep.data.last[4], 'close mapped from c'

    recent = @exchange.get_candles(ticker: ticker, start_at: 30.days.ago, timeframe: 1.day)
    assert_operator recent.data.size, :<=, 31, 'a recent start trims the window'
  end

  test 'get_candles builds one_month from daily instead of sending the minute-colliding 1M scale' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT')
    now = Time.now.utc
    daily = (0...400).map { |i| { 'is' => (now - i.days).to_i * 1000, 'o' => '100', 'h' => '120', 'l' => '90', 'c' => '110', 'v' => '5' } }
    requested = nil
    client = Object.new
    client.define_singleton_method(:candlestick_data) do |**kw|
      requested = kw[:interval]
      Result::Success.new({ 'data' => daily })
    end
    @exchange.stubs(:client).returns(client)

    result = @exchange.get_candles(ticker: ticker, start_at: 20.years.ago, timeframe: 1.month)
    assert_predicate result, :success?
    assert_equal '1d', requested, "must fetch daily and build monthly, never send '1M' (= one minute on Bitrue)"
    assert result.data.any?
    assert_operator result.data.size, :<, 400, 'monthly aggregation reduces the daily count'
  end

  test 'get_candles returns a failure for an unsupported timeframe' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT')
    @exchange.expects(:client).never
    result = @exchange.get_candles(ticker: ticker, start_at: 1.day.ago, timeframe: 2.hours)
    assert_predicate result, :failure?
  end

  test 'get_candles returns a failure when a built timeframe has no source candles' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT')
    client = Object.new
    client.define_singleton_method(:candlestick_data) { |**_kw| Result::Success.new({ 'data' => [] }) }
    @exchange.stubs(:client).returns(client)
    result = @exchange.get_candles(ticker: ticker, start_at: 20.years.ago, timeframe: 1.month)
    assert_predicate result, :failure?
  end

  # ---- balances ----
  # Bitrue names assets in lower case ("btc"; https://github.com/ccxt/ccxt python/ccxt/bitrue.py
  # parse_balance capture, and every baseAsset in the live exchangeInfo), while tickers store them upper.

  test 'get_balances reads lower-case asset codes' do
    Rails.configuration.stubs(:dry_run).returns(false)
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT')
    client = Object.new
    client.define_singleton_method(:account_information) do
      Result::Success.new({ 'canTrade' => true, 'balances' => [
                            { 'asset' => 'btc', 'free' => '0.5', 'locked' => '0.1' },
                            { 'asset' => 'usdt', 'free' => '20', 'locked' => '0' }
                          ] })
    end
    @exchange.stubs(:client).returns(client)

    result = @exchange.get_balances(asset_ids: [ticker.base_asset_id, ticker.quote_asset_id])
    assert_predicate result, :success?
    assert_equal({ free: '0.5'.to_d, locked: '0.1'.to_d }, result.data[ticker.base_asset_id])
    assert_equal({ free: 20.to_d, locked: 0.to_d }, result.data[ticker.quote_asset_id])
  end

  # ---- orders (GET /api/v1/order, https://github.com/Bitrue-exchange/Spot-official-api-docs#query-order-user_data) ----

  def stub_query_order(raw)
    client = Object.new
    client.define_singleton_method(:query_order) do |symbol:, order_id:|
      raw.is_a?(Result::Failure) ? raw : Result::Success.new({ order_id: "#{symbol}-#{order_id}", raw: raw })
    end
    @exchange.stubs(:client).returns(client)
  end

  def order_raw(**fields)
    { 'symbol' => 'BTCUSDT', 'orderId' => 42, 'clientOrderId' => 'x', 'price' => '0', 'origQty' => '0.002',
      'executedQty' => '0', 'cummulativeQuoteQty' => '0', 'status' => 'NEW', 'timeInForce' => 'GTC',
      'type' => 'LIMIT', 'side' => 'BUY', 'time' => 1_499_827_319_559, 'updateTime' => 1_499_827_319_559 }
      .merge(fields.transform_keys(&:to_s))
  end

  test 'a filled limit order is priced at its fills, not at its limit' do
    Rails.configuration.stubs(:dry_run).returns(false)
    create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT')
    stub_query_order(order_raw(price: '50000', executedQty: '0.002', cummulativeQuoteQty: '98', status: 'FILLED'))

    result = @exchange.get_order(order_id: 'BTCUSDT-42')
    assert_predicate result, :success?
    assert_equal 49_000.to_d, result.data[:price]
    assert_equal :closed, result.data[:status]
  end

  test 'an order still queued (PENDING_CREATE) reads as open instead of raising' do
    Rails.configuration.stubs(:dry_run).returns(false)
    create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT')
    stub_query_order(order_raw(status: 'PENDING_CREATE'))

    assert_equal :open, @exchange.get_order(order_id: 'BTCUSDT-42').data[:status]
  end

  test 'an order Bitrue no longer has goes under missing instead of failing the batch' do
    Rails.configuration.stubs(:dry_run).returns(false)
    create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT')
    client = Object.new
    client.define_singleton_method(:query_order) do |symbol:, order_id:|
      raw = if order_id == '7'
              { 'code' => -2013, 'msg' => 'Could not find order information for given order ID.' }
            else
              { 'symbol' => symbol, 'orderId' => order_id.to_i, 'price' => '0', 'origQty' => '0.002',
                'executedQty' => '0.002', 'cummulativeQuoteQty' => '100', 'status' => 'FILLED',
                'type' => 'MARKET', 'side' => 'BUY' }
            end
      Result::Success.new({ order_id: "#{symbol}-#{order_id}", raw: raw })
    end
    @exchange.stubs(:client).returns(client)

    result = @exchange.get_orders(order_ids: %w[BTCUSDT-7 BTCUSDT-8])
    assert_predicate result, :success?
    assert_equal ['BTCUSDT-7'], result.data[:missing]
    assert_equal ['BTCUSDT-8'], result.data[:orders].keys
  end
end
