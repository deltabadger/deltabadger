require 'test_helper'

# A request that got no HTTP answer reaches the app two ways. An app client (Clients::Alpaca,
# Clients::Ibkr, and every client on Client#with_rescue) handles the exception itself; a honeymaker
# venue hands back a Result carrying the same exception chain as data[:error_chain]. Both have to give
# the same two answers: may a read be retried, and did a failed placement provably never leave (so it
# can be written down as failed) or may it be on the book?
class TransportFailureTest < ActiveSupport::TestCase
  PROXY = '203.0.113.10:9100'.freeze
  VENUE = '203.0.113.20:443'.freeze
  TLS_ERROR = "SSL_connect returned=1 errno=0 peeraddr=#{VENUE} state=error: ".freeze

  # Raises inner, then outer while handling it, so Ruby links them through #cause the way an adapter
  # re-raises. Faraday's own wrappers link through #wrapped_exception instead.
  def self.caused(outer, inner)
    raise inner
  rescue StandardError
    begin
      raise outer
    rescue StandardError => e
      e
    end
  end

  # name => [the exception as the adapter raises it, the chain honeymaker reports for it,
  #          transient? (a read retries), pre-transmission? (a failed placement is definitive)]
  def self.shapes
    {
      'a bare timeout' => [
        Faraday::TimeoutError.new('execution expired'),
        %w[Faraday::TimeoutError], true, false
      ],
      'a bare refused connection' => [
        Faraday::ConnectionFailed.new(Net::HTTP::Persistent::Error.new("connection refused: #{PROXY}")),
        %w[Faraday::ConnectionFailed Net::HTTP::Persistent::Error], true, true
      ],
      'a refused connection' => [
        Faraday::ConnectionFailed.new(caused(Net::HTTP::Persistent::Error.new("connection refused: #{PROXY}"),
                                             Errno::ECONNREFUSED.new("connect(2) for #{PROXY}"))),
        %w[Faraday::ConnectionFailed Net::HTTP::Persistent::Error Errno::ECONNREFUSED], true, true
      ],
      'a DNS failure' => [
        Faraday::ConnectionFailed.new(
          Socket::ResolutionError.new('Failed to open TCP connection to api.example.com:443 ' \
                                      '(getaddrinfo(3): nodename nor servname provided, or not known)')
        ),
        %w[Faraday::ConnectionFailed Socket::ResolutionError], true, true
      ],
      'a proxy refusing CONNECT' => [
        Faraday::ConnectionFailed.new(Net::HTTPClientException.new('407 "Proxy Authentication Required"', nil)),
        %w[Faraday::ConnectionFailed Net::HTTPClientException], false, true
      ],
      'a closed connection' => [
        Faraday::ConnectionFailed.new(EOFError.new('end of file reached')),
        %w[Faraday::ConnectionFailed EOFError], true, false
      ],
      'a TLS certificate failure' => [
        Faraday::SSLError.new(OpenSSL::SSL::SSLError.new(
                                "#{TLS_ERROR}certificate verify failed (unable to get local issuer certificate)"
                              )),
        %w[Faraday::SSLError OpenSSL::SSL::SSLError], false, false
      ],
      'a TLS connection closed mid-handshake' => [
        Faraday::SSLError.new(OpenSSL::SSL::SSLError.new("#{TLS_ERROR}unexpected eof while reading")),
        %w[Faraday::SSLError OpenSSL::SSL::SSLError], true, false
      ],
      'a connect timeout' => [
        Faraday::TimeoutError.new(caused(Net::OpenTimeout.new("Failed to open TCP connection to #{VENUE} (execution expired)"),
                                         Errno::ETIMEDOUT.new)),
        %w[Faraday::TimeoutError Net::OpenTimeout Errno::ETIMEDOUT], true, true
      ],
      'a read timeout' => [
        Faraday::TimeoutError.new(Net::ReadTimeout.new),
        %w[Faraday::TimeoutError Net::ReadTimeout], true, false
      ],
      'an aborted connection' => [
        Faraday::ConnectionFailed.new(Errno::ECONNABORTED.new),
        %w[Faraday::ConnectionFailed Errno::ECONNABORTED], true, false
      ],
      'no free local address' => [
        Faraday::ConnectionFailed.new(Errno::EADDRNOTAVAIL.new),
        %w[Faraday::ConnectionFailed Errno::EADDRNOTAVAIL], true, true
      ],
      'a connection the host refuses to permit' => [
        Faraday::ConnectionFailed.new(Errno::EACCES.new),
        %w[Faraday::ConnectionFailed Errno::EACCES], false, false
      ]
    }
  end

  setup do
    @bot = create(:dca_single_asset, :started)
    @bot.stubs(:ensure_exchange_authenticated)
    @ticker = @bot.tickers.first
    @exchange = @bot.exchange
  end

  shapes.each_key do |name|
    test "#{name}: the chain honeymaker reports is the one the app walks" do
      error, chain = self.class.shapes.fetch(name)

      assert_equal chain, Client.error_chain(error)
    end

    test "#{name}: a read through an app client" do
      error, chain, transient = self.class.shapes.fetch(name)

      [Client.new, Clients::Alpaca.new(api_key: 'k', api_secret: 's', paper: true),
       Clients::Ibkr.new(api_key: nil)].each do |client|
        outcome = begin
          client.send(:with_rescue) { raise error }
        rescue Client::TransientNetworkError => e
          e
        end

        if transient
          assert_kind_of Client::TransientNetworkError, outcome, "#{client.class} retries it"
        else
          assert_kind_of Result::Failure, outcome, "#{client.class} surfaces it instead of retrying"
          assert_equal ["#{error.class}: #{error.message}"], outcome.errors
          assert_equal({ status: nil, error_chain: chain }, outcome.data)
          refute @exchange.transient_failure?(outcome), 'and nothing downstream retries it either'
        end
      end
    end

    test "#{name}: a read through a honeymaker venue" do
      error, chain, transient = self.class.shapes.fetch(name)

      assert_equal transient, @exchange.transient_failure?(honeymaker_failure(error, chain))
    end

    test "#{name}: a placement through an app client" do
      error, _chain, _transient, pre_transmission = self.class.shapes.fetch(name)
      @exchange.define_singleton_method(:market_buy) { |**| Client.new.with_rescue { raise error } }

      outcome = begin
        @bot.market_buy(ticker: @ticker, amount: 10, amount_type: :quote)
      rescue Client::TransientNetworkError, Client::AmbiguousPlacementError => e
        e
      end

      ambiguous = outcome.is_a?(Client::AmbiguousPlacementError) ||
                  (outcome.is_a?(Result) && @exchange.ambiguous_placement_error?(outcome))
      assert_equal !pre_transmission, ambiguous
    end

    test "#{name}: a placement through a honeymaker venue" do
      error, chain, _transient, pre_transmission = self.class.shapes.fetch(name)

      assert_equal !pre_transmission, @exchange.ambiguous_placement_error?(honeymaker_failure(error, chain))
    end
  end

  # The released gem reports no chain, and an adapter can rebuild a failure as bare text. Those keep
  # the text rules, which now also know the two ways a dropped connection reads. Text is the weaker
  # signal, so it stays conservative: a DNS message alone is not retried, a DNS chain is.
  test 'without a chain, a failure is classified by its text' do
    closed = Result::Failure.new('end of file reached', data: { status: nil })
    tls_closed = Result::Failure.new("#{TLS_ERROR}unexpected eof while reading", data: { status: nil })
    dns = Result::Failure.new('Failed to open TCP connection to api.example.com:443 (getaddrinfo(3): ' \
                              'nodename nor servname provided, or not known)', data: { status: nil })

    assert @exchange.transient_failure?(closed)
    assert @exchange.transient_failure?(tls_closed)
    refute @exchange.transient_failure?(dns)
    assert @exchange.ambiguous_placement_error?(closed), 'a dropped connection may have carried the order'
  end

  private

  # What honeymaker hands back for a request with no HTTP answer: the exception's own message.
  def honeymaker_failure(error, chain)
    Result::Failure.new(error.message, data: { status: nil, error_chain: chain })
  end
end
