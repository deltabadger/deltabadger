require 'test_helper'

class Clients::LaunchpadTest < ActiveSupport::TestCase
  setup do
    @client = Clients::Launchpad.new
  end

  test 'claim posts the code and returns the symbolized payload' do
    payload = {
      'identity' => { 'email' => 'owner@example.com', 'name' => 'Owner' },
      'market_data' => {
        'url' => 'https://market-data.example.com',
        'token' => 'dbi_token',
        'provider_name' => 'Deltabadger'
      },
      'proxies' => { 'Binance' => 'https://proxy.example.com' }
    }
    connection = stub
    connection.expects(:post).with('api/claim', { code: 'dbc_instance_secret' }).returns(stub(body: payload))
    @client.stubs(:connection).returns(connection)

    result = @client.claim('dbc_instance_secret')

    assert_predicate result, :success?
    assert_equal 'owner@example.com', result.data.dig(:identity, :email)
    assert_equal 'dbi_token', result.data.dig(:market_data, :token)
    assert_equal({ Binance: 'https://proxy.example.com' }, result.data[:proxies])
  end

  test 'claim preserves the raw 401 response for the service layer to classify' do
    body = '{"error":"invalid_or_expired"}'
    connection = stub
    connection.stubs(:post).raises(Faraday::ClientError.new('unauthorized', { status: 401, body: body }))
    @client.stubs(:connection).returns(connection)

    result = @client.claim('dbc_invalid')

    assert_predicate result, :failure?
    assert_equal [body], result.errors
    assert_equal({ status: 401 }, result.data)
  end

  test 'claim tells the user a read timeout may have consumed the one-time code' do
    connection = stub
    timeout = Faraday::TimeoutError.new('execution expired')
    timeout.stubs(:cause).returns(Net::ReadTimeout.new('read timed out'))
    connection.stubs(:post).raises(timeout)
    @client.stubs(:connection).returns(connection)

    result = @client.claim('dbc_instance_secret')

    assert_predicate result, :failure?
    assert_equal [Clients::Launchpad::AMBIGUOUS_CLAIM_MESSAGE], result.errors
  end

  test 'claim says to retry after a definitely pre-send connection failure' do
    connection = stub
    failure = Faraday::ConnectionFailed.new('connection refused')
    failure.stubs(:cause).returns(Errno::ECONNREFUSED.new)
    connection.stubs(:post).raises(failure)
    @client.stubs(:connection).returns(connection)

    result = @client.claim('dbc_instance_secret')

    assert_predicate result, :failure?
    assert_equal [Clients::Launchpad::RETRYABLE_CLAIM_MESSAGE], result.errors
  end

  test 'claim says to retry after an open timeout' do
    connection = stub
    timeout = Faraday::TimeoutError.new('open timed out')
    timeout.stubs(:cause).returns(Net::OpenTimeout.new('open timed out'))
    connection.stubs(:post).raises(timeout)
    @client.stubs(:connection).returns(connection)

    result = @client.claim('dbc_instance_secret')

    assert_predicate result, :failure?
    assert_equal [Clients::Launchpad::RETRYABLE_CLAIM_MESSAGE], result.errors
  end

  test 'claim says to retry after a DNS failure' do
    connection = stub
    failure = Faraday::ConnectionFailed.new('name resolution failed')
    failure.stubs(:cause).returns(SocketError.new('getaddrinfo failed'))
    connection.stubs(:post).raises(failure)
    @client.stubs(:connection).returns(connection)

    result = @client.claim('dbc_instance_secret')

    assert_predicate result, :failure?
    assert_equal [Clients::Launchpad::RETRYABLE_CLAIM_MESSAGE], result.errors
  end

  test 'uses the launchpad-specific connection timeouts' do
    assert_equal({ request: { open_timeout: 5, read_timeout: 10 } }, Clients::Launchpad::OPTIONS)
  end
end
