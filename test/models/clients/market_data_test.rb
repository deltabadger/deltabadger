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
end
