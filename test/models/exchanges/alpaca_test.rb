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

    result = @exchange.send(:set_market_order, ticker: ticker, amount: 100.12345, amount_type: :quote, side: :buy)
    assert_predicate result, :success?
  end

  test 'set_market_order rounds qty to base_decimals when amount_type is base' do
    btc = Asset.find_by(symbol: 'BTC') || create(:asset, :bitcoin)
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    ticker = create(:ticker, exchange: @exchange, base_asset: btc, quote_asset: usd, base_decimals: 8)

    Clients::Alpaca.any_instance.stubs(:create_order).with do |params|
      params[:qty] == '0.12345678' && params[:type] == 'market'
    end.returns(Result::Success.new({ 'id' => 'order-1' }))

    result = @exchange.send(:set_market_order, ticker: ticker, amount: 0.123456789, amount_type: :base, side: :buy)
    assert_predicate result, :success?
  end

  # == set_limit_order ==

  test 'set_limit_order rounds qty to base_decimals' do
    btc = Asset.find_by(symbol: 'BTC') || create(:asset, :bitcoin)
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    ticker = create(:ticker, exchange: @exchange, base_asset: btc, quote_asset: usd, base_decimals: 8, price_decimals: 2)

    Clients::Alpaca.any_instance.stubs(:create_order).with do |params|
      params[:qty] == '0.00200000' && params[:type] == 'limit'
    end.returns(Result::Success.new({ 'id' => 'order-2' }))

    result = @exchange.send(:set_limit_order, ticker: ticker, amount: 100.12345, amount_type: :quote, side: :buy, price: 50_000.0)
    assert_predicate result, :success?
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
    create(:ticker, exchange: @exchange, base_asset: btc, quote_asset: usd)

    snapshot_data = { 'BTC' => { 'latestTrade' => { 'p' => 50_000.0 } } }
    Clients::Alpaca.any_instance.stubs(:get_snapshots).with(symbols: %w[BTC])
                   .returns(Result::Success.new(snapshot_data))

    result = @exchange.get_tickers_prices
    assert_predicate result, :success?
    assert_equal 50_000.0.to_d, result.data['BTC']
  end
end
