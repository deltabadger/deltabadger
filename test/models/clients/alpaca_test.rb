require 'test_helper'

class Clients::AlpacaTest < ActiveSupport::TestCase
  setup do
    @client = Clients::Alpaca.new(api_key: 'test_key', api_secret: 'test_secret')
  end

  test 'initializes with api_key and api_secret' do
    assert_equal 'test_key', @client.instance_variable_get(:@api_key)
    assert_equal 'test_secret', @client.instance_variable_get(:@api_secret)
    assert_equal false, @client.instance_variable_get(:@paper)
  end

  test 'initializes with paper trading mode' do
    client = Clients::Alpaca.new(api_key: 'key', api_secret: 'secret', paper: true)
    assert_equal true, client.instance_variable_get(:@paper)
  end

  test 'get_account returns success result' do
    mock_response = stub(body: { 'id' => '123', 'status' => 'ACTIVE', 'cash' => '10000.00' })
    connection = stub
    connection.expects(:get).yields(stub_request('/v2/account')).returns(mock_response)
    @client.stubs(:trading_connection).returns(connection)

    result = @client.get_account
    assert_predicate result, :success?
    assert_equal '123', result.data['id']
  end

  test 'get_positions returns success result' do
    mock_response = stub(body: [{ 'symbol' => 'AAPL', 'qty' => '10' }])
    connection = stub
    connection.expects(:get).yields(stub_request('/v2/positions')).returns(mock_response)
    @client.stubs(:trading_connection).returns(connection)

    result = @client.get_positions
    assert_predicate result, :success?
    assert_equal 'AAPL', result.data[0]['symbol']
  end

  test 'get_assets returns success result' do
    mock_response = stub(body: [{ 'symbol' => 'AAPL', 'name' => 'Apple Inc' }])
    connection = stub
    connection.expects(:get).yields(stub_request('/v2/assets')).returns(mock_response)
    @client.stubs(:trading_connection).returns(connection)

    result = @client.get_assets
    assert_predicate result, :success?
    assert_equal 'AAPL', result.data[0]['symbol']
  end

  test 'get_asset returns success result' do
    mock_response = stub(body: { 'symbol' => 'AAPL', 'name' => 'Apple Inc', 'exchange' => 'NASDAQ' })
    connection = stub
    connection.expects(:get).yields(stub_request('/v2/assets/AAPL')).returns(mock_response)
    @client.stubs(:trading_connection).returns(connection)

    result = @client.get_asset(symbol: 'AAPL')
    assert_predicate result, :success?
    assert_equal 'AAPL', result.data['symbol']
  end

  test 'create_order returns success result' do
    mock_response = stub(body: { 'id' => 'order-123', 'status' => 'accepted' })
    connection = stub
    connection.expects(:post).yields(stub_request('/v2/orders')).returns(mock_response)
    @client.stubs(:trading_connection).returns(connection)

    result = @client.create_order(symbol: 'AAPL', side: 'buy', type: 'market', time_in_force: 'day', notional: '100')
    assert_predicate result, :success?
    assert_equal 'order-123', result.data['id']
  end

  test 'get_order returns success result' do
    mock_response = stub(body: { 'id' => 'order-123', 'status' => 'filled' })
    connection = stub
    connection.expects(:get).yields(stub_request('/v2/orders/order-123')).returns(mock_response)
    @client.stubs(:trading_connection).returns(connection)

    result = @client.get_order(order_id: 'order-123')
    assert_predicate result, :success?
    assert_equal 'filled', result.data['status']
  end

  test 'cancel_order returns success result' do
    mock_response = stub(body: nil)
    connection = stub
    connection.expects(:delete).yields(stub_request('/v2/orders/order-123')).returns(mock_response)
    @client.stubs(:trading_connection).returns(connection)

    result = @client.cancel_order(order_id: 'order-123')
    assert_predicate result, :success?
  end

  test 'get_latest_quote returns success result' do
    mock_response = stub(body: { 'quote' => { 'ap' => 150.25, 'bp' => 150.20 } })
    connection = stub
    connection.expects(:get).yields(stub_request('/v2/stocks/AAPL/quotes/latest')).returns(mock_response)
    @client.stubs(:data_connection).returns(connection)

    result = @client.get_latest_quote(symbol: 'AAPL')
    assert_predicate result, :success?
  end

  test 'get_latest_trade returns success result' do
    mock_response = stub(body: { 'trade' => { 'p' => 150.22 } })
    connection = stub
    connection.expects(:get).yields(stub_request('/v2/stocks/AAPL/trades/latest')).returns(mock_response)
    @client.stubs(:data_connection).returns(connection)

    result = @client.get_latest_trade(symbol: 'AAPL')
    assert_predicate result, :success?
  end

  test 'get_clock returns success result' do
    mock_response = stub(body: { 'is_open' => true, 'next_open' => '2026-03-09T09:30:00-04:00' })
    connection = stub
    connection.expects(:get).yields(stub_request('/v2/clock')).returns(mock_response)
    @client.stubs(:trading_connection).returns(connection)

    result = @client.get_clock
    assert_predicate result, :success?
    assert_equal true, result.data['is_open']
  end

  test 'get_bars returns success result' do
    mock_response = stub(body: { 'bars' => [{ 'o' => 150.0, 'h' => 155.0, 'l' => 149.0, 'c' => 154.0 }] })
    connection = stub
    connection.expects(:get).yields(stub_request('/v2/stocks/AAPL/bars')).returns(mock_response)
    @client.stubs(:data_connection).returns(connection)

    result = @client.get_bars(symbol: 'AAPL', timeframe: '1Day')
    assert_predicate result, :success?
  end

  test 'handles Faraday errors gracefully' do
    connection = stub
    connection.stubs(:get).raises(Faraday::ClientError.new('Forbidden', { status: 403, body: 'Forbidden' }))
    @client.stubs(:trading_connection).returns(connection)

    result = @client.get_account
    assert_predicate result, :failure?
  end

  private

  def stub_request(_url)
    req = stub
    req.stubs(:url=)
    req.stubs(:url)
    req.stubs(:headers=)
    req.stubs(:headers)
    req.stubs(:params=)
    req.stubs(:params)
    req.stubs(:body=)
    req.stubs(:body)
    req
  end
end
