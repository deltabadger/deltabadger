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

  test 'claim converts a timeout into a failure result' do
    connection = stub
    connection.stubs(:post).raises(Faraday::TimeoutError, 'execution expired')
    @client.stubs(:connection).returns(connection)

    result = @client.claim('dbc_instance_secret')

    assert_predicate result, :failure?
    assert_equal ['Unable to reach Deltabadger. Please try again.'], result.errors
  end

  test 'uses the launchpad-specific connection timeouts' do
    assert_equal({ request: { open_timeout: 5, read_timeout: 10 } }, Clients::Launchpad::OPTIONS)
  end
end
