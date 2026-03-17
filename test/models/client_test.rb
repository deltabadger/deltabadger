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
    error = Faraday::ConnectionFailed.new('connection timed out')

    result = @client.with_rescue { raise error }

    assert_predicate result, :failure?
    assert_equal ['connection timed out'], result.errors
  end
end
