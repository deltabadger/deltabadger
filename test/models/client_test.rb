require 'test_helper'

class ClientTest < ActiveSupport::TestCase
  setup do
    @client = Client.new
  end

  test 'with_rescue strips HTML response body and uses HTTP status code' do
    html_body = '<html><head><title>401 Authorization Required</title></head><body></body></html>'
    error = Faraday::ClientError.new(nil, { status: 401, body: html_body })

    result = @client.with_rescue { raise error }

    assert_predicate result, :failure?
    assert_equal ['HTTP 401'], result.errors
  end

  test 'with_rescue preserves non-HTML error messages' do
    error = Faraday::ClientError.new(nil, { status: 400, body: 'Insufficient buying power' })

    result = @client.with_rescue { raise error }

    assert_predicate result, :failure?
    assert_equal ['Insufficient buying power'], result.errors
  end

  test 'with_rescue falls back to exception message when body is empty' do
    # Use a non-transient Faraday error here; Faraday::ConnectionFailed is now
    # re-raised as Client::TransientNetworkError (see tests below).
    error = Faraday::ServerError.new('upstream down', { status: 502, body: nil })

    result = @client.with_rescue { raise error }

    assert_predicate result, :failure?
    assert_equal ['upstream down'], result.errors
  end

  test 'with_rescue re-raises Net::OpenTimeout as Client::TransientNetworkError' do
    err = assert_raises(Client::TransientNetworkError) do
      @client.with_rescue { raise Net::OpenTimeout, 'TCP open timed out' }
    end
    assert_match(/Net::OpenTimeout/, err.message)
    assert_match(/TCP open timed out/, err.message)
  end

  test 'with_rescue re-raises Faraday::ConnectionFailed as Client::TransientNetworkError' do
    err = assert_raises(Client::TransientNetworkError) do
      @client.with_rescue { raise Faraday::ConnectionFailed, 'connection refused' }
    end
    assert_match(/Faraday::ConnectionFailed/, err.message)
    assert_match(/connection refused/, err.message)
  end

  test 'with_rescue re-raises Faraday::TimeoutError as Client::TransientNetworkError' do
    err = assert_raises(Client::TransientNetworkError) do
      @client.with_rescue { raise Faraday::TimeoutError, 'read timed out' }
    end
    assert_match(/Faraday::TimeoutError/, err.message)
  end

  test 'with_rescue still returns Result::Failure for non-transient Faraday errors' do
    error = Faraday::ClientError.new('bad request', { status: 400, body: 'nope' })

    result = @client.with_rescue { raise error }

    assert_predicate result, :failure?
    assert_equal ['nope'], result.errors
  end

  test 'with_rescue still returns Result::Failure for generic StandardError' do
    result = @client.with_rescue { raise StandardError, 'something else' }

    assert_predicate result, :failure?
    assert_equal ['something else'], result.errors
  end
end
