require 'test_helper'

class Clients::MarketDataTest < ActiveSupport::TestCase
  setup do
    @client = Clients::MarketData.new(url: 'http://data-api:3000', token: 'tok')
  end

  test 'get_stocks calls api/v2/assets with stock/etf type and identifiers include' do
    mock_response = stub(body: {
                           'metadata' => { 'count' => 1 },
                           'data' => [
                             { 'external_id' => 'AAPL.US', 'type' => 'stock', 'symbol' => 'AAPL', 'name' => 'Apple',
                               'color' => '#000000',
                               'identifiers' => [{ 'scheme' => 'alpaca', 'value' => 'us_equity:AAPL' }] }
                           ]
                         })
    v2 = stub
    v2.expects(:get).with('api/v2/assets', { type: 'stock,etf', include: 'identifiers' }).returns(mock_response)
    @client.stubs(:v2_connection).returns(v2)

    result = @client.get_stocks
    assert_predicate result, :success?
    assert_equal 'AAPL.US', result.data['data'].first['external_id']
    assert_equal 'us_equity:AAPL', result.data['data'].first['identifiers'].first['value']
  end

  test 'get_stocks returns Result::Failure on Faraday error' do
    v2 = stub
    v2.stubs(:get).raises(Faraday::ClientError.new('boom', { status: 500, body: 'oops' }))
    @client.stubs(:v2_connection).returns(v2)

    result = @client.get_stocks
    assert_predicate result, :failure?
  end

  test 'get_alpaca_listings calls api/v2/listings with venue_scheme=alpaca_exchange' do
    mock_response = stub(body: {
                           'metadata' => { 'count' => 1 },
                           'data' => [
                             { 'listing_id' => 'NASDAQ:AAPL', 'base' => 'AAPL', 'quote' => 'USD', 'ticker' => 'AAPL',
                               'base_external_id' => 'AAPL.US', 'quote_external_id' => 'USD.FOREX',
                               'fractionable' => true }
                           ]
                         })
    v2 = stub
    v2.expects(:get).with('api/v2/listings', { venue_scheme: 'alpaca_exchange' }).returns(mock_response)
    @client.stubs(:v2_connection).returns(v2)

    result = @client.get_alpaca_listings
    assert_predicate result, :success?
    listing = result.data['data'].first
    assert_equal 'NASDAQ:AAPL', listing['listing_id']
    assert_equal 'AAPL.US', listing['base_external_id']
    assert_equal 'USD.FOREX', listing['quote_external_id']
  end

  test 'get_alpaca_listings returns Result::Failure on Faraday error' do
    v2 = stub
    v2.stubs(:get).raises(Faraday::ClientError.new('boom', { status: 500, body: 'oops' }))
    @client.stubs(:v2_connection).returns(v2)

    result = @client.get_alpaca_listings
    assert_predicate result, :failure?
  end

  test 'get_alpaca_crypto_listings fetches the alpaca_crypto venue listings' do
    mock_response = stub(body: { 'metadata' => { 'count' => 1 }, 'data' => [
                           { 'listing_id' => 'alpaca_crypto:AAVE/USD', 'venue' => 'alpaca_crypto', 'venue_scheme' => 'exchange_slug',
                             'symbol' => 'AAVE/USD', 'native_symbol' => 'AAVE/USD',
                             'base_asset_id' => 'crypto:aave', 'quote_asset_id' => 'fiat:USD',
                             'base_decimals' => 8, 'quote_decimals' => 2, 'price_decimals' => 2,
                             'minimum_base_size' => '0.01', 'minimum_quote_size' => '1',
                             'maximum_base_size' => '100000', 'maximum_quote_size' => '200000',
                             'fractionable' => true, 'trading_enabled' => true }
                         ] })
    v2 = stub
    v2.expects(:get).with('api/v2/listings', { venue: 'alpaca_crypto' }).returns(mock_response)
    @client.stubs(:v2_connection).returns(v2)

    result = @client.get_alpaca_crypto_listings
    assert_predicate result, :success?
    assert_equal 1, result.data['data'].size
  end

  # --- Bulk READ timeout ------------------------------------------------------------------
  #
  # Faraday resolves the socket read timeout as `options[:read_timeout] || options[:timeout]`
  # (Faraday::Adapter#request_timeout). Client::OPTIONS sets a CONNECTION-level read_timeout of
  # 30s, so a per-request `req.options.timeout = 60` is never consulted and the request still dies
  # at 30s — which is exactly what production did every day at 10:00 UTC: 38 of 92 containers hit
  # Net::ReadTimeout at 30.1s while BULK_READ_TIMEOUT claimed to allow 60. These assert the
  # EFFECTIVE value under Faraday's own resolution rule, not merely that some option was set.

  BULK_ENDPOINTS = {
    get_stocks: '/api/v2/assets',
    get_alpaca_listings: '/api/v2/listings',
    get_alpaca_crypto_listings: '/api/v2/listings'
  }.freeze

  BULK_ENDPOINTS.each do |method, path|
    test "#{method} widens the effective read timeout to BULK_READ_TIMEOUT" do
      env = capture_request_env(path) { @client.public_send(method) }

      assert_equal Clients::MarketData::BULK_READ_TIMEOUT, effective_read_timeout(env)
    end
  end

  # Guards the test above from passing for the wrong reason: if the connection-level read_timeout
  # were ever dropped from Client::OPTIONS, a per-request `timeout` would start working again and
  # the bulk assertions would pass while trading clients silently lost their tighter budget.
  test 'non-bulk reads keep the tighter global read timeout' do
    env = capture_request_env('/api/v1/exchange_rates') { @client.get_exchange_rates }

    assert_equal 30, Client::OPTIONS.dig(:request, :read_timeout)
    assert_equal Client::OPTIONS.dig(:request, :read_timeout), effective_read_timeout(env)
  end

  # Indices consumption is migrated to the v2 surface (v1/indices is being sunset).
  test 'get_indices calls api/v2/indices' do
    mock_response = stub(body: { 'metadata' => { 'count' => 1 },
                                 'data' => [{ 'external_id' => 'nasdaq-100', 'source' => 'deltabadger',
                                              'top_coins' => ['AAPL.US'], 'weights' => { 'AAPL.US' => 9.12 } }] })
    v2 = stub
    v2.expects(:get).with('api/v2/indices').returns(mock_response)
    @client.stubs(:v2_connection).returns(v2)

    result = @client.get_indices
    assert_predicate result, :success?
    assert_equal 'nasdaq-100', result.data['data'].first['external_id']
  end

  private

  # Mirrors Faraday::Adapter#request_timeout(:read, options).
  def effective_read_timeout(env)
    env.request[:read_timeout] || env.request[:timeout]
  end

  # A real Faraday stack (so per-request option merging happens for real) carrying the same
  # connection-level Client::OPTIONS as Clients::MarketData#build_connection, with the network
  # swapped for the test adapter.
  def capture_request_env(path)
    captured = nil
    stubs = Faraday::Adapter::Test::Stubs.new do |stub|
      stub.get(path) do |env|
        captured = env
        [200, { 'Content-Type' => 'application/json' }, '{"metadata":{"count":0},"data":[]}']
      end
    end
    connection = Faraday.new(url: 'http://data-api:3000', **Client::OPTIONS) do |config|
      config.response :json
      config.adapter :test, stubs
    end
    @client.stubs(:connection).returns(connection)
    @client.stubs(:v2_connection).returns(connection)

    yield

    stubs.verify_stubbed_calls
    captured
  end
end
