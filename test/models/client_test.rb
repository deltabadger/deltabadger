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

  # The venue answered and the JSON middleware could not read the answer. For a placement that is
  # an accepted order with no acknowledgement, so the failure has to say what kind it is.
  test 'with_rescue marks a response it could not parse as unreadable, with its status' do
    error = Faraday::ParsingError.new(JSON::ParserError.new("unexpected token at '{\"id\":'"),
                                      { status: 200, body: '{"id":' })

    result = @client.with_rescue { raise error }

    assert result.failure?
    assert_equal 'Unreadable response (HTTP 200)', result.errors.first
    assert result.data[:unreadable]
    assert_equal 200, result.data[:status]
  end

  # The walk and the preference were extracted so a chain honeymaker reports can be classified the same
  # way. Whatever a chain resolved to before must resolve to the same thing now.
  test 'specific_cause_name is unchanged for the chains it already handled' do
    open_timeout = caused(Net::OpenTimeout.new('execution expired'), Errno::ETIMEDOUT.new)
    refused = Faraday::ConnectionFailed.new(
      caused(Net::HTTP::Persistent::Error.new('connection refused: 203.0.113.10:9100'), Errno::ECONNREFUSED.new)
    )

    assert_equal 'Net::OpenTimeout', Client.specific_cause_name(open_timeout)
    assert_equal 'Errno::ECONNREFUSED', Client.specific_cause_name(refused)
    assert_equal 'Faraday::TimeoutError', Client.specific_cause_name(Faraday::TimeoutError.new('execution expired'))
  end

  test 'most_specific_cause names what a wrapper hides, and the wrapper when nothing does' do
    assert_equal 'EOFError', Client.most_specific_cause(%w[Faraday::ConnectionFailed EOFError])
    assert_equal 'OpenSSL::SSL::SSLError', Client.most_specific_cause(%w[Faraday::SSLError OpenSSL::SSL::SSLError])
    assert_equal 'Net::HTTPClientException',
                 Client.most_specific_cause(%w[Faraday::ConnectionFailed Net::HTTPClientException])
    assert_equal 'Faraday::ConnectionFailed',
                 Client.most_specific_cause(%w[Faraday::ConnectionFailed Net::HTTP::Persistent::Error])
  end

  test 'with_rescue returns a TLS failure that cannot be retried instead of raising it' do
    error = Faraday::SSLError.new(OpenSSL::SSL::SSLError.new('certificate verify failed'))

    result = @client.with_rescue { raise error }

    assert_predicate result, :failure?
    assert_equal ['Faraday::SSLError: certificate verify failed'], result.errors
    assert_equal({ status: nil, error_chain: %w[Faraday::SSLError OpenSSL::SSL::SSLError] }, result.data)
  end

  test 'with_rescue still returns Result::Failure for generic StandardError' do
    result = @client.with_rescue { raise StandardError, 'something else' }

    assert_predicate result, :failure?
    assert_equal ['something else'], result.errors
  end

  private

  # Raises inner, then outer while handling it, so Ruby links them through #cause.
  def caused(outer, inner)
    raise inner
  rescue StandardError
    begin
      raise outer
    rescue StandardError => e
      e
    end
  end

  # An adapter can re-wrap in the class it already used; the cause beneath the second instance still
  # counts. Before, a class seen once stopped the walk there and ECONNREFUSED went missing.
  test 'error_chain follows a class that wraps itself' do
    error = Faraday::ConnectionFailed.new(Faraday::ConnectionFailed.new(Errno::ECONNREFUSED.new))

    assert_equal %w[Faraday::ConnectionFailed Errno::ECONNREFUSED], Client.error_chain(error)
  end
end
