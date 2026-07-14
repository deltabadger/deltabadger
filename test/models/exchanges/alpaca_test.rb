require 'test_helper'

class Exchanges::AlpacaTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:alpaca_exchange)
  end

  test 'coingecko_id returns nil' do
    assert_nil @exchange.coingecko_id
  end

  test 'known_errors includes insufficient_funds' do
    errors = @exchange.known_errors
    assert errors[:insufficient_funds].present?
    assert_kind_of Array, errors[:insufficient_funds]
  end

  test 'minimum_amount_logic returns quote' do
    assert_equal :quote, @exchange.minimum_amount_logic
  end

  test 'supports_withdrawal? returns false' do
    refute_predicate @exchange, :supports_withdrawal?
  end

  test 'requires_passphrase? returns true for mode selection' do
    assert_predicate @exchange, :requires_passphrase?
  end

  test 'set_client creates Clients::Alpaca instance' do
    api_key = stub(key: 'test_key', secret: 'test_secret', passphrase: nil)
    @exchange.set_client(api_key: api_key)
    assert_kind_of Clients::Alpaca, @exchange.instance_variable_get(:@client)
  end

  test 'set_client sets api_key reader' do
    api_key = stub(key: 'test_key', secret: 'test_secret', passphrase: nil)
    @exchange.set_client(api_key: api_key)
    assert_equal api_key, @exchange.api_key
  end

  test 'set_client handles nil api_key' do
    @exchange.set_client(api_key: nil)
    assert_nil @exchange.api_key
    assert_kind_of Clients::Alpaca, @exchange.instance_variable_get(:@client)
  end

  test 'market_open? returns true when clock says open' do
    Clients::Alpaca.any_instance.stubs(:get_clock).returns(Result::Success.new({ 'is_open' => true, 'next_open' => 1.hour.from_now.iso8601 }))
    assert_predicate @exchange, :market_open?
  end

  test 'market_open? returns false when clock says closed' do
    Clients::Alpaca.any_instance.stubs(:get_clock).returns(Result::Success.new({ 'is_open' => false, 'next_open' => 1.hour.from_now.iso8601 }))
    refute_predicate @exchange, :market_open?
  end

  test 'market_open? returns true when clock request fails' do
    Clients::Alpaca.any_instance.stubs(:get_clock).returns(Result::Failure.new('connection error'))
    assert_predicate @exchange, :market_open?
  end

  test 'set_client defaults to paper mode when passphrase is nil' do
    api_key = stub(key: 'test_key', secret: 'test_secret', passphrase: nil)
    @exchange.set_client(api_key: api_key)
    client = @exchange.instance_variable_get(:@client)
    assert client.instance_variable_get(:@paper)
  end

  test 'set_client uses live mode when passphrase is live' do
    api_key = stub(key: 'test_key', secret: 'test_secret', passphrase: 'live')
    @exchange.set_client(api_key: api_key)
    client = @exchange.instance_variable_get(:@client)
    refute client.instance_variable_get(:@paper)
  end

  test 'list_open_orders returns parsed orders' do
    btc = Asset.find_by(symbol: 'BTC') || create(:asset, :bitcoin)
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    create(:ticker, exchange: @exchange, base_asset: btc, quote_asset: usd)

    raw_orders = [
      { 'id' => 'uuid-1', 'symbol' => 'BTC', 'side' => 'buy', 'type' => 'limit', 'status' => 'new',
        'qty' => '0.5', 'limit_price' => '50000', 'filled_qty' => '0', 'filled_avg_price' => nil,
        'notional' => nil }
    ]
    Clients::Alpaca.any_instance.stubs(:list_orders).returns(Result::Success.new(raw_orders))

    result = @exchange.list_open_orders
    assert_predicate result, :success?
    assert_equal 1, result.data.size
    assert_equal 'uuid-1', result.data[0][:order_id]
    assert_equal :buy, result.data[0][:side]
    assert_equal :limit_order, result.data[0][:order_type]
  end

  test 'list_open_orders returns failure when API fails' do
    Clients::Alpaca.any_instance.stubs(:list_orders).returns(Result::Failure.new('connection error'))

    result = @exchange.list_open_orders
    assert_predicate result, :failure?
  end

  test 'parse_order_data handles nil filled_avg_price and nil limit_price' do
    btc = Asset.find_by(symbol: 'BTC') || create(:asset, :bitcoin)
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    unless Ticker.exists?(exchange: @exchange, base_asset: btc, quote_asset: usd)
      create(:ticker, exchange: @exchange, base_asset: btc, quote_asset: usd)
    end

    order_data = {
      'id' => 'uuid-1', 'symbol' => 'BTC', 'side' => 'buy', 'type' => 'market', 'status' => 'accepted',
      'qty' => '1.0', 'limit_price' => nil, 'filled_qty' => '0', 'filled_avg_price' => nil,
      'notional' => '50000'
    }

    parsed = @exchange.send(:parse_order_data, order_data)
    assert_equal 0, parsed[:price]
  end

  # == set_market_order ==

  test 'set_market_order rounds notional to quote_decimals when amount_type is quote' do
    btc = Asset.find_by(symbol: 'BTC') || create(:asset, :bitcoin)
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    ticker = create(:ticker, exchange: @exchange, base_asset: btc, quote_asset: usd, quote_decimals: 2)

    Clients::Alpaca.any_instance.stubs(:create_order).with do |params|
      params[:notional] == '100.12' && params[:type] == 'market'
    end.returns(Result::Success.new({ 'id' => 'order-1' }))

    result = with_dry_run(false) do
      @exchange.send(:set_market_order, ticker: ticker, amount: 100.12345, amount_type: :quote, side: :buy)
    end
    assert_predicate result, :success?
  end

  test 'set_market_order pads notional to quote_decimals when amount floors to whole number' do
    btc = Asset.find_by(symbol: 'BTC') || create(:asset, :bitcoin)
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    ticker = create(:ticker, exchange: @exchange, base_asset: btc, quote_asset: usd, quote_decimals: 2)

    Clients::Alpaca.any_instance.stubs(:create_order).with do |params|
      params[:notional] == '40.00' && params[:type] == 'market'
    end.returns(Result::Success.new({ 'id' => 'order-1' }))

    result = with_dry_run(false) do
      @exchange.send(:set_market_order, ticker: ticker, amount: BigDecimal('40.00000002825'), amount_type: :quote, side: :buy)
    end
    assert_predicate result, :success?
  end

  test 'set_market_order rounds qty to base_decimals when amount_type is base' do
    btc = Asset.find_by(symbol: 'BTC') || create(:asset, :bitcoin)
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    ticker = create(:ticker, exchange: @exchange, base_asset: btc, quote_asset: usd, base_decimals: 8)

    Clients::Alpaca.any_instance.stubs(:create_order).with do |params|
      params[:qty] == '0.12345678' && params[:type] == 'market'
    end.returns(Result::Success.new({ 'id' => 'order-1' }))

    result = with_dry_run(false) do
      @exchange.send(:set_market_order, ticker: ticker, amount: 0.123456789, amount_type: :base, side: :buy)
    end
    assert_predicate result, :success?
  end

  # == set_limit_order ==

  test 'set_limit_order rounds qty to base_decimals' do
    btc = Asset.find_by(symbol: 'BTC') || create(:asset, :bitcoin)
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    ticker = create(:ticker, exchange: @exchange, base_asset: btc, quote_asset: usd, base_decimals: 8, price_decimals: 2)

    Clients::Alpaca.any_instance.stubs(:create_order).with do |params|
      params[:qty] == '0.00200240' && params[:type] == 'limit'
    end.returns(Result::Success.new({ 'id' => 'order-2' }))

    result = with_dry_run(false) do
      @exchange.send(:set_limit_order, ticker: ticker, amount: 100.12345, amount_type: :quote, side: :buy, price: 50_000.0)
    end
    assert_predicate result, :success?
  end

  # == order symbol + time_in_force: pair symbol and gtc for crypto, bare symbol and day for stocks ==

  test 'set_market_order sends the pair symbol and gtc time_in_force for a crypto ticker' do
    aave = Asset.find_by(external_id: 'aave') || create(:asset, external_id: 'aave', symbol: 'AAVE', category: 'Cryptocurrency')
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    ticker = create(:ticker, exchange: @exchange, base_asset: aave, quote_asset: usd, ticker: 'AAVE/USD',
                             base_decimals: 8, quote_decimals: 2)

    Clients::Alpaca.any_instance.stubs(:create_order).with do |params|
      params[:symbol] == 'AAVE/USD' && params[:time_in_force] == 'gtc'
    end.returns(Result::Success.new({ 'id' => 'order-1' }))

    result = with_dry_run(false) do
      @exchange.send(:set_market_order, ticker: ticker, amount: 100, amount_type: :quote, side: :buy)
    end
    assert_predicate result, :success?
  end

  test 'set_market_order sends the bare symbol and day time_in_force for a stock ticker (regression)' do
    aapl = create(:asset, external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', category: 'Stock')
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    ticker = create(:ticker, exchange: @exchange, base_asset: aapl, quote_asset: usd, ticker: 'AAPL',
                             base_decimals: 9, quote_decimals: 2)

    Clients::Alpaca.any_instance.stubs(:create_order).with do |params|
      params[:symbol] == 'AAPL' && params[:time_in_force] == 'day'
    end.returns(Result::Success.new({ 'id' => 'order-1' }))

    result = with_dry_run(false) do
      @exchange.send(:set_market_order, ticker: ticker, amount: 100, amount_type: :quote, side: :buy)
    end
    assert_predicate result, :success?
  end

  test 'set_limit_order sends the pair symbol and gtc time_in_force for a crypto ticker' do
    aave = Asset.find_by(external_id: 'aave') || create(:asset, external_id: 'aave', symbol: 'AAVE', category: 'Cryptocurrency')
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    ticker = create(:ticker, exchange: @exchange, base_asset: aave, quote_asset: usd, ticker: 'AAVE/USD',
                             base_decimals: 8, price_decimals: 2)

    Clients::Alpaca.any_instance.stubs(:create_order).with do |params|
      params[:symbol] == 'AAVE/USD' && params[:type] == 'limit' && params[:time_in_force] == 'gtc'
    end.returns(Result::Success.new({ 'id' => 'order-2' }))

    result = with_dry_run(false) do
      @exchange.send(:set_limit_order, ticker: ticker, amount: 1, amount_type: :base, side: :buy, price: 100.0)
    end
    assert_predicate result, :success?
  end

  test 'parse_order_data looks the ticker up by ticker.ticker (pair symbol for crypto)' do
    aave = Asset.find_by(external_id: 'aave') || create(:asset, external_id: 'aave', symbol: 'AAVE', category: 'Cryptocurrency')
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    ticker = create(:ticker, exchange: @exchange, base_asset: aave, quote_asset: usd, ticker: 'AAVE/USD')

    order_data = {
      'id' => 'uuid-2', 'symbol' => 'AAVE/USD', 'side' => 'buy', 'type' => 'market', 'status' => 'filled',
      'qty' => '1.0', 'limit_price' => nil, 'filled_qty' => '1.0', 'filled_avg_price' => '100.0',
      'notional' => nil
    }

    parsed = @exchange.send(:parse_order_data, order_data)
    assert_equal ticker, parsed[:ticker]
  end

  # == get_last_price / get_bid_price / get_ask_price / get_candles (crypto branch) ==

  test 'get_last_price uses the crypto latest-trade endpoint for a crypto ticker' do
    aave = Asset.find_by(external_id: 'aave') || create(:asset, external_id: 'aave', symbol: 'AAVE', category: 'Cryptocurrency')
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    ticker = create(:ticker, exchange: @exchange, base_asset: aave, quote_asset: usd, ticker: 'AAVE/USD')

    Clients::Alpaca.any_instance.stubs(:get_crypto_latest_trade).with(symbols: ['AAVE/USD'])
                   .returns(Result::Success.new({ 'trades' => { 'AAVE/USD' => { 'p' => 100.5 } } }))

    result = @exchange.get_last_price(ticker: ticker)
    assert_predicate result, :success?
    assert_equal 100.5.to_d, result.data
  end

  test 'get_last_price still uses the stock latest-trade endpoint for a stock ticker (regression)' do
    aapl = create(:asset, external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', category: 'Stock')
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    ticker = create(:ticker, exchange: @exchange, base_asset: aapl, quote_asset: usd, ticker: 'AAPL')

    Clients::Alpaca.any_instance.stubs(:get_latest_trade).with(symbol: 'AAPL')
                   .returns(Result::Success.new({ 'trade' => { 'p' => 150.0 } }))

    result = @exchange.get_last_price(ticker: ticker)
    assert_predicate result, :success?
    assert_equal 150.0.to_d, result.data
  end

  test 'get_bid_price uses the crypto latest-quote endpoint for a crypto ticker' do
    aave = Asset.find_by(external_id: 'aave') || create(:asset, external_id: 'aave', symbol: 'AAVE', category: 'Cryptocurrency')
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    ticker = create(:ticker, exchange: @exchange, base_asset: aave, quote_asset: usd, ticker: 'AAVE/USD')

    Clients::Alpaca.any_instance.stubs(:get_crypto_latest_quote).with(symbols: ['AAVE/USD'])
                   .returns(Result::Success.new({ 'quotes' => { 'AAVE/USD' => { 'bp' => 99.5, 'ap' => 100.5 } } }))

    result = @exchange.get_bid_price(ticker: ticker)
    assert_predicate result, :success?
    assert_equal 99.5.to_d, result.data
  end

  test 'get_ask_price uses the crypto latest-quote endpoint for a crypto ticker' do
    aave = Asset.find_by(external_id: 'aave') || create(:asset, external_id: 'aave', symbol: 'AAVE', category: 'Cryptocurrency')
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    ticker = create(:ticker, exchange: @exchange, base_asset: aave, quote_asset: usd, ticker: 'AAVE/USD')

    Clients::Alpaca.any_instance.stubs(:get_crypto_latest_quote).with(symbols: ['AAVE/USD'])
                   .returns(Result::Success.new({ 'quotes' => { 'AAVE/USD' => { 'bp' => 99.5, 'ap' => 100.5 } } }))

    result = @exchange.get_ask_price(ticker: ticker)
    assert_predicate result, :success?
    assert_equal 100.5.to_d, result.data
  end

  test 'get_candles uses the crypto bars endpoint for a crypto ticker' do
    aave = Asset.find_by(external_id: 'aave') || create(:asset, external_id: 'aave', symbol: 'AAVE', category: 'Cryptocurrency')
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    ticker = create(:ticker, exchange: @exchange, base_asset: aave, quote_asset: usd, ticker: 'AAVE/USD')

    Clients::Alpaca.any_instance.stubs(:get_crypto_bars).with do |params|
      params[:symbol] == 'AAVE/USD' && params[:timeframe] == '1Day'
    end.returns(Result::Success.new({ 'bars' => { 'AAVE/USD' => [
                                      { 't' => '2026-01-01T00:00:00Z', 'o' => '1', 'h' => '2', 'l' => '0.5', 'c' => '1.5', 'v' => '10' }
                                    ] } }))

    result = @exchange.get_candles(ticker: ticker, start_at: 1.day.ago, timeframe: 1.day)
    assert_predicate result, :success?
    assert_equal 1, result.data.size
  end

  # == get_tickers_prices (bulk metrics pricing — partitions stock vs. crypto symbols) ==

  test 'get_tickers_prices routes crypto pair symbols to the crypto endpoint and merges with stock snapshots' do
    Clients::Alpaca.any_instance.stubs(:get_snapshots).with(symbols: ['AAPL'])
                   .returns(Result::Success.new({ 'AAPL' => { 'latestTrade' => { 'p' => 150.0 } } }))
    Clients::Alpaca.any_instance.stubs(:get_crypto_latest_trade).with(symbols: ['AAVE/USD'])
                   .returns(Result::Success.new({ 'trades' => { 'AAVE/USD' => { 'p' => 100.5 } } }))

    result = @exchange.get_tickers_prices(symbols: ['AAPL', 'AAVE/USD'])
    assert_predicate result, :success?
    assert_equal 150.0.to_d, result.data['AAPL']
    assert_equal 100.5.to_d, result.data['AAVE/USD']
  end

  test 'get_tickers_prices with only crypto symbols never calls the stock snapshots endpoint' do
    Clients::Alpaca.any_instance.expects(:get_snapshots).never
    Clients::Alpaca.any_instance.stubs(:get_crypto_latest_trade).with(symbols: ['AAVE/USD'])
                   .returns(Result::Success.new({ 'trades' => { 'AAVE/USD' => { 'p' => 100.5 } } }))

    result = @exchange.get_tickers_prices(symbols: ['AAVE/USD'])
    assert_predicate result, :success?
    assert_equal 100.5.to_d, result.data['AAVE/USD']
  end

  test 'fetch_withdrawal_fees! returns empty success' do
    result = @exchange.fetch_withdrawal_fees!
    assert_predicate result, :success?
    assert_equal({}, result.data)
  end

  # == get_tickers_prices with symbols parameter ==

  test 'get_tickers_prices with symbols fetches only requested symbols via snapshots' do
    snapshot_data = {
      'AAPL' => { 'latestTrade' => { 'p' => 150.25 } },
      'MSFT' => { 'latestTrade' => { 'p' => 310.50 } }
    }
    Clients::Alpaca.any_instance.stubs(:get_snapshots).with(symbols: %w[AAPL MSFT])
                   .returns(Result::Success.new(snapshot_data))

    result = @exchange.get_tickers_prices(symbols: %w[AAPL MSFT])
    assert_predicate result, :success?
    assert_equal 150.25.to_d, result.data['AAPL']
    assert_equal 310.50.to_d, result.data['MSFT']
  end

  test 'get_tickers_prices with symbols returns failure when snapshots fail' do
    Clients::Alpaca.any_instance.stubs(:get_snapshots)
                   .returns(Result::Failure.new('connection error'))

    result = @exchange.get_tickers_prices(symbols: %w[AAPL])
    assert_predicate result, :failure?
  end

  test 'get_tickers_prices with symbols uses different cache keys for different symbols' do
    snapshot_aapl = { 'AAPL' => { 'latestTrade' => { 'p' => 150.0 } } }
    snapshot_msft = { 'MSFT' => { 'latestTrade' => { 'p' => 310.0 } } }
    Clients::Alpaca.any_instance.stubs(:get_snapshots).with(symbols: %w[AAPL])
                   .returns(Result::Success.new(snapshot_aapl))
    Clients::Alpaca.any_instance.stubs(:get_snapshots).with(symbols: %w[MSFT])
                   .returns(Result::Success.new(snapshot_msft))

    result1 = @exchange.get_tickers_prices(symbols: %w[AAPL])
    result2 = @exchange.get_tickers_prices(symbols: %w[MSFT])

    assert_equal({ 'AAPL' => 150.0.to_d }, result1.data)
    assert_equal({ 'MSFT' => 310.0.to_d }, result2.data)
  end

  test 'get_tickers_prices without symbols falls back to all tickers' do
    btc = Asset.find_by(symbol: 'BTC') || create(:asset, :bitcoin)
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    create(:ticker, exchange: @exchange, base_asset: btc, quote_asset: usd, ticker: 'BTC')

    snapshot_data = { 'BTC' => { 'latestTrade' => { 'p' => 50_000.0 } } }
    Clients::Alpaca.any_instance.stubs(:get_snapshots).with(symbols: %w[BTC])
                   .returns(Result::Success.new(snapshot_data))

    result = @exchange.get_tickers_prices
    assert_predicate result, :success?
    assert_equal 50_000.0.to_d, result.data['BTC']
  end

  # == get_orders shape contract ==
  # Per-ID-loop pattern: Alpaca's REST API surfaces a per-order error if an ID is
  # unknown, so the loop bails on failure rather than silently dropping. The
  # contract is the same { orders:, missing: } shape, with missing always empty.

  test 'get_orders returns { orders:, missing: [] } shape' do
    base = begin
      create(:asset, :stock_aapl)
    rescue StandardError
      create(:asset, symbol: 'AAPL', external_id: 'apple', name: 'Apple')
    end
    quote = create(:asset, :usd)
    create(:ticker, exchange: @exchange, base_asset: base, quote_asset: quote, base: 'AAPL', quote: 'USD', ticker: 'AAPL')
    api_key = stub(key: 'k', secret: 's', passphrase: 'live')
    @exchange.set_client(api_key: api_key)

    Clients::Alpaca.any_instance.stubs(:get_order).returns(
      Result::Success.new(
        'id' => 'order-1', 'symbol' => 'AAPL', 'type' => 'market', 'side' => 'buy',
        'status' => 'filled', 'qty' => '1', 'filled_qty' => '1', 'filled_avg_price' => '100',
        'notional' => '100', 'limit_price' => nil
      )
    )

    result = with_dry_run(false) do
      @exchange.get_orders(order_ids: %w[order-1 order-2])
    end

    assert result.success?
    assert_equal %i[orders missing].sort, result.data.keys.sort
    assert_kind_of Hash, result.data[:orders]
    assert_equal [], result.data[:missing]
  end

  # --- Market-data authentication ---
  # Regression: Exchanges::Alpaca#client builds an empty-credential client unless
  # set_client(api_key:) was called. The dashboard metric/chart reads never set one, so
  # Alpaca market data (data.alpaca.markets) 401'd on every load — nothing cached, every
  # page load retried the failing call. Market-data reads now resolve a trading key via a
  # NON-MUTATING client, so account ops can never inherit that key.

  def trading_key!(key: 'mk', secret: 'ms', passphrase: 'paper', status: :correct)
    create(:api_key, exchange: @exchange, key_type: :trading, status: status,
                     raw_key: key, raw_secret: secret, raw_passphrase: passphrase)
  end

  test 'get_tickers_prices authenticates with a resolved trading key when no client was set' do
    trading_key!(key: 'mk', secret: 'ms', passphrase: 'paper')
    snapshot = { 'AAPL' => { 'latestTrade' => { 'p' => 100 } } }
    Clients::Alpaca.expects(:new).with(api_key: 'mk', api_secret: 'ms', paper: true)
                   .returns(stub(get_snapshots: Result::Success.new(snapshot)))

    result = @exchange.get_tickers_prices(symbols: ['AAPL'])

    assert_predicate result, :success?
    assert_equal 100.to_d, result.data['AAPL']
  end

  test 'get_candles authenticates with a resolved trading key when no client was set' do
    trading_key!(key: 'mk', secret: 'ms', passphrase: 'paper')
    stock_asset = create(:asset, category: 'Stock')
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    ticker = create(:ticker, exchange: @exchange, base_asset: stock_asset, quote_asset: usd)
    Clients::Alpaca.expects(:new).with(api_key: 'mk', api_secret: 'ms', paper: true)
                   .returns(stub(get_bars: Result::Success.new({ 'bars' => [] })))

    result = @exchange.get_candles(ticker: ticker, start_at: 1.day.ago, timeframe: 1.day)

    assert_predicate result, :success?
  end

  test 'market-data reads are non-mutating: @api_key and @client stay untouched' do
    trading_key!
    Clients::Alpaca.stubs(:new).returns(stub(get_snapshots: Result::Success.new({})))

    @exchange.get_tickers_prices(symbols: ['AAPL'])

    assert_nil @exchange.api_key
    assert_nil @exchange.instance_variable_get(:@client)
  end

  test 'market-data reads prefer an explicitly set api_key over a resolved one' do
    trading_key!(key: 'resolved', secret: 'rs', passphrase: 'paper')
    explicit = stub(key: 'explicit', secret: 'es', passphrase: 'live')
    explicit_client = mock('explicit_client')
    explicit_client.expects(:get_snapshots).returns(Result::Success.new({}))
    Clients::Alpaca.stubs(:new).with(api_key: 'explicit', api_secret: 'es', paper: false)
                   .returns(explicit_client)
    Clients::Alpaca.stubs(:new).with(api_key: 'resolved', api_secret: 'rs', paper: true)
                   .returns(stub(get_snapshots: Result::Success.new({})))

    @exchange.set_client(api_key: explicit)
    @exchange.get_tickers_prices(symbols: ['AAPL'], force: true)
  end

  test 'account ops use the generic (uncredentialed) client, never the resolved market-data key' do
    trading_key!(key: 'mk', secret: 'ms')
    Clients::Alpaca.expects(:new).with(api_key: nil, api_secret: nil, paper: true)
                   .returns(stub(get_account: Result::Failure.new('unauthorized')))

    with_dry_run(false) { @exchange.get_balances }
  end

  test 'market-data read with no trading key fails gracefully' do
    Clients::Alpaca.expects(:new).with(api_key: nil, api_secret: nil, paper: true)
                   .returns(stub(get_snapshots: Result::Failure.new('unauthorized')))

    result = @exchange.get_tickers_prices(symbols: ['AAPL'])

    assert_predicate result, :failure?
  end
end
