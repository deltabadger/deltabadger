require 'test_helper'

class Clients::IbkrTest < ActiveSupport::TestCase
  self.use_transactional_tests = false # IbkrLock relies on committed rows

  setup do
    @api_key = Struct.new(:key).new('CONSUMERKEY')
    @session = mock('session')
    @client = Clients::Ibkr.new(api_key: @api_key, session: @session)
  end

  teardown { IbkrLock.delete_all }

  test 'wraps calls in IbkrLock keyed by a fingerprint of the consumer key' do
    expected_key = "ibkr:#{Digest::SHA256.hexdigest('CONSUMERKEY')}"
    IbkrLock.expects(:with_lock).with(expected_key).yields.returns(Result::Success.new([]))
    @session.stubs(:signed_request).returns([])

    @client.accounts
  end

  test 'accounts returns a Result::Success wrapping the body' do
    @session.expects(:signed_request).with(:get, '/v1/api/iserver/accounts').returns(['U123'])
    result = @client.accounts
    assert_predicate result, :success?
    assert_equal ['U123'], result.data
  end

  test 'place_order answers a confirmation prompt then returns the order ack' do
    # first POST returns a confirmation prompt; the reply returns the accepted order
    @session.expects(:signed_request)
            .with(:post, '/v1/api/iserver/account/U123/orders', body: anything)
            .returns([{ 'id' => 'abc', 'message' => ['Are you sure?'] }])
    @session.expects(:signed_request)
            .with(:post, '/v1/api/iserver/reply/abc', body: { confirmed: true })
            .returns([{ 'order_id' => '777', 'order_status' => 'Submitted' }])

    result = @client.place_order(account_id: 'U123', conid: 265_598, side: :buy, quantity: 3)

    assert_predicate result, :success?
    assert_equal '777', result.data.first['order_id']
  end

  test 'search_contract picks the conid matching the requested currency' do
    @session.expects(:signed_request)
            .with(:get, '/v1/api/iserver/secdef/search', query: { symbol: 'VUSA' })
            .returns([
                       { 'conid' => 111, 'currency' => 'USD', 'sections' => [{ 'secType' => 'STK' }] },
                       { 'conid' => 222, 'currency' => 'GBP', 'sections' => [{ 'secType' => 'STK' }] }
                     ])

    result = @client.search_contract(symbol: 'VUSA', currency: 'GBP')

    assert_predicate result, :success?
    assert_equal 222, result.data
  end

  test 'search_contract fails cleanly when no conid is found' do
    @session.expects(:signed_request).returns([])
    result = @client.search_contract(symbol: 'NOPE', currency: 'USD')
    assert_predicate result, :failure?
  end

  test 'translates a Faraday error into a Result::Failure with the IBKR message' do
    err = Faraday::BadRequestError.new('400', { status: 400, body: '{"error":"Order value exceeds limit"}' })
    @session.expects(:signed_request).raises(err)

    result = @client.accounts

    assert_predicate result, :failure?
    assert_equal 'Order value exceeds limit', result.errors.first
  end

  test 're-raises transient network errors for the caller to retry' do
    @session.expects(:signed_request).raises(Faraday::TimeoutError.new('read timed out'))
    assert_raises(Client::TransientNetworkError) { @client.accounts }
  end
end
