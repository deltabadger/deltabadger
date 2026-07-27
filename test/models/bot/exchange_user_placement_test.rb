require 'test_helper'

# A network error raised DURING ORDER PLACEMENT is not safely retryable. The exchange may have
# accepted the order before the response timed out, and placement carries no idempotency key,
# so replaying the job places the order a second time.
#
# This is a LATENT hazard, not a postmortem. A 2026-07-27 fleet scan found 24 close-together
# duplicate placements, but forensics attributed them to user-initiated restarts (a start places
# immediately), and 14 of them predate `retry_on Client::TransientNetworkError` landing in
# Bot::ActionJob at all (99a4558f8, 2026-05-28). The replay below has never been observed firing —
# it is guarded because it is reachable in code and spends real money when it does.
#
# Chain: Client#with_rescue RAISES Client::TransientNetworkError on Faraday::TimeoutError /
# Net::OpenTimeout / Faraday::ConnectionFailed (client.rb) -> no exchange model rescues it around
# placement -> Bot::ActionJob's `retry_on Client::TransientNetworkError, attempts: 4` replays the
# whole job.
#
# Exchange::PLACEMENT_SAFE_TRANSIENT_ERRORS already encodes exactly this reasoning ("a placement
# network timeout ... is indistinguishable from a successful book hit ... it must NOT be treated as
# safely transient"), but it only guards the Result::Failure path. These tests cover the RAISE path.
class Bot::ExchangeUserPlacementTest < ActiveSupport::TestCase
  setup do
    @bot = create(:dca_single_asset, :started)
    @bot.stubs(:ensure_exchange_authenticated)
    @ticker = @bot.tickers.first
    @timeout = Client::TransientNetworkError.new('Faraday::TimeoutError: read timeout reached')
  end

  # --- the class contract that makes the whole fix work -------------------------------------

  # Load-bearing: Bot::ActionJob declares `retry_on Client::TransientNetworkError`, and retry_on
  # matches subclasses. If AmbiguousPlacementError ever inherits from TransientNetworkError the
  # replay comes straight back, silently.
  test 'AmbiguousPlacementError is NOT a TransientNetworkError' do
    refute Client::AmbiguousPlacementError.ancestors.include?(Client::TransientNetworkError),
           'AmbiguousPlacementError must not subclass TransientNetworkError — retry_on matches ' \
           'subclasses, which would re-enable the double-buy replay'
    assert Client::AmbiguousPlacementError.ancestors.include?(StandardError)
  end

  # --- placement converts, so the job cannot replay it --------------------------------------

  test 'market_buy converts a placement network timeout into AmbiguousPlacementError' do
    @bot.exchange.stubs(:market_buy).raises(@timeout)

    assert_raises(Client::AmbiguousPlacementError) do
      @bot.market_buy(ticker: @ticker, amount: 10, amount_type: :quote)
    end
  end

  test 'market_sell converts a placement network timeout into AmbiguousPlacementError' do
    @bot.exchange.stubs(:market_sell).raises(@timeout)

    assert_raises(Client::AmbiguousPlacementError) do
      @bot.market_sell(ticker: @ticker, amount: 10, amount_type: :quote)
    end
  end

  test 'limit_buy converts a placement network timeout into AmbiguousPlacementError' do
    @bot.exchange.stubs(:limit_buy).raises(@timeout)

    assert_raises(Client::AmbiguousPlacementError) do
      @bot.limit_buy(ticker: @ticker, amount: 10, amount_type: :quote, price: 100)
    end
  end

  test 'limit_sell converts a placement network timeout into AmbiguousPlacementError' do
    @bot.exchange.stubs(:limit_sell).raises(@timeout)

    assert_raises(Client::AmbiguousPlacementError) do
      @bot.limit_sell(ticker: @ticker, amount: 10, amount_type: :quote, price: 100)
    end
  end

  test 'the converted error preserves the original message for diagnosis' do
    @bot.exchange.stubs(:market_buy).raises(@timeout)

    error = assert_raises(Client::AmbiguousPlacementError) do
      @bot.market_buy(ticker: @ticker, amount: 10, amount_type: :quote)
    end
    assert_match(/Faraday::TimeoutError: read timeout reached/, error.message)
  end

  # --- failures that provably happened BEFORE the request was sent stay retryable ------------
  # Ambiguity is the whole justification for refusing the retry. A connection that was never
  # established carries no ambiguity: nothing reached the exchange, so retrying cannot double-buy.
  # Converting those too would cost a bot a full interval — up to a month — every time the AWS
  # exchange proxy is down, which is a documented recurring outage (Errno::ECONNREFUSED on :8100).

  test 'a connect timeout during placement stays retryable' do
    @bot.exchange.stubs(:market_buy).raises(
      Client::TransientNetworkError.new('Net::OpenTimeout: execution expired',
                                        original_class: 'Net::OpenTimeout')
    )

    assert_raises(Client::TransientNetworkError) do
      @bot.market_buy(ticker: @ticker, amount: 10, amount_type: :quote)
    end
  end

  test 'a refused connection during placement stays retryable' do
    @bot.exchange.stubs(:market_buy).raises(
      Client::TransientNetworkError.new('Faraday::ConnectionFailed: connection refused: 18.132.181.159:8100',
                                        original_class: 'Errno::ECONNREFUSED')
    )

    assert_raises(Client::TransientNetworkError) do
      @bot.market_buy(ticker: @ticker, amount: 10, amount_type: :quote)
    end
  end

  # Ruby 3.3+ raises DNS failures as Socket::ResolutionError, a SUBCLASS of SocketError. An exact
  # 'SocketError' name match misses it, so a transient DNS outage would skip a monthly bot's whole
  # interval instead of retrying safely.
  test 'a DNS resolution failure during placement stays retryable' do
    @bot.exchange.stubs(:market_buy).raises(
      Client::TransientNetworkError.new('Faraday::ConnectionFailed: Failed to open TCP connection',
                                        original_class: 'Socket::ResolutionError')
    )

    assert_raises(Client::TransientNetworkError) do
      @bot.market_buy(ticker: @ticker, amount: 10, amount_type: :quote)
    end
  end

  # Classification is an ALLOWLIST of failures that PROVE nothing was transmitted, and it fails
  # SAFE: an errno nobody listed is treated as possibly-landed and costs one skipped tick, rather
  # than being retried and possibly spending twice. A denylist was tried and rejected for inverting
  # exactly that asymmetry (Errno::ECONNABORTED can fire on an established socket).
  test 'a local address failure during placement stays retryable' do
    @bot.exchange.stubs(:market_buy).raises(
      Client::TransientNetworkError.new('Faraday::ConnectionFailed: cannot assign requested address',
                                        original_class: 'Errno::EADDRNOTAVAIL')
    )

    assert_raises(Client::TransientNetworkError) do
      @bot.market_buy(ticker: @ticker, amount: 10, amount_type: :quote)
    end
  end

  test 'an unlisted errno converts — unknown provenance must fail safe' do
    @bot.exchange.stubs(:market_buy).raises(
      Client::TransientNetworkError.new('Faraday::ConnectionFailed: software caused connection abort',
                                        original_class: 'Errno::ECONNABORTED')
    )

    assert_raises(Client::AmbiguousPlacementError) do
      @bot.market_buy(ticker: @ticker, amount: 10, amount_type: :quote)
    end
  end

  test 'a broken pipe during placement converts — bytes may already be on the wire' do
    @bot.exchange.stubs(:market_buy).raises(
      Client::TransientNetworkError.new('Faraday::ConnectionFailed: broken pipe',
                                        original_class: 'Errno::EPIPE')
    )

    assert_raises(Client::AmbiguousPlacementError) do
      @bot.market_buy(ticker: @ticker, amount: 10, amount_type: :quote)
    end
  end

  test 'a bare socket-level timeout during placement converts' do
    @bot.exchange.stubs(:market_buy).raises(
      Client::TransientNetworkError.new('Faraday::ConnectionFailed: connection timed out',
                                        original_class: 'Errno::ETIMEDOUT')
    )

    assert_raises(Client::AmbiguousPlacementError) do
      @bot.market_buy(ticker: @ticker, amount: 10, amount_type: :quote)
    end
  end

  test 'a read timeout during placement converts — the request was already sent' do
    @bot.exchange.stubs(:market_buy).raises(
      Client::TransientNetworkError.new('Faraday::TimeoutError: read timeout reached',
                                        original_class: 'Faraday::TimeoutError')
    )

    assert_raises(Client::AmbiguousPlacementError) do
      @bot.market_buy(ticker: @ticker, amount: 10, amount_type: :quote)
    end
  end

  # A connection reset can happen after the request went out, so it is genuinely ambiguous —
  # unlike a refusal, which means the connection was never accepted in the first place.
  test 'a connection reset during placement converts' do
    @bot.exchange.stubs(:market_buy).raises(
      Client::TransientNetworkError.new('Faraday::ConnectionFailed: Connection reset by peer',
                                        original_class: 'Errno::ECONNRESET')
    )

    assert_raises(Client::AmbiguousPlacementError) do
      @bot.market_buy(ticker: @ticker, amount: 10, amount_type: :quote)
    end
  end

  # Other code paths raise Client::TransientNetworkError directly without provenance. Unknown
  # provenance must mean "assume the request may have landed" — the safe default is never to buy
  # twice, at the cost of one skipped tick.
  test 'a transient error of unknown provenance converts' do
    @bot.exchange.stubs(:market_buy).raises(Client::TransientNetworkError.new('something broke'))

    assert_raises(Client::AmbiguousPlacementError) do
      @bot.market_buy(ticker: @ticker, amount: 10, amount_type: :quote)
    end
  end

  # Every with_rescue must record provenance, not just the base one. Alpaca and IBKR override it,
  # and a nil original_class means "unknown", which the guard conservatively treats as ambiguous —
  # so an override that drops provenance silently converts even a definitive connect timeout into
  # a skipped interval. Alpaca matters especially: one of the verified production double-buys was
  # an Alpaca bot.
  test 'Clients::Alpaca#with_rescue records provenance' do
    client = Clients::Alpaca.new(api_key: 'k', api_secret: 's', paper: true)

    error = assert_raises(Client::TransientNetworkError) do
      client.send(:with_rescue) { raise Net::OpenTimeout, 'execution expired' }
    end
    assert_equal 'Net::OpenTimeout', error.original_class
  end

  test 'Clients::Ibkr#with_rescue records provenance' do
    client = Clients::Ibkr.new(api_key: nil)

    error = assert_raises(Client::TransientNetworkError) do
      client.send(:with_rescue) { raise Net::OpenTimeout, 'execution expired' }
    end
    assert_equal 'Net::OpenTimeout', error.original_class
  end

  # The net_http_persistent adapter (used by Clients::Alpaca) wraps ECONNREFUSED in its own
  # Net::HTTP::Persistent::Error before Faraday wraps that, so wrapped_exception alone stops one
  # level short and reports the adapter wrapper. Exchange::NETWORK_TRANSIENT_PATTERNS already
  # carries a comment about this exact adapter quirk biting a previous fix. Getting it wrong here
  # means a dead exchange proxy — the documented recurring outage — reads as ambiguous and costs
  # every Alpaca bot a full interval instead of a safe retry.
  test 'Client#with_rescue walks nested causes to the real errno' do
    client = Class.new(Client) do
      def boom
        with_rescue do
          raise Errno::ECONNREFUSED, 'connection refused'
        rescue StandardError
          # net_http_persistent's wrapper, which Faraday then wraps again.
          raise Faraday::ConnectionFailed, RuntimeError.new('connection refused: 1.2.3.4:8100')
        end
      end
    end.new

    error = assert_raises(Client::TransientNetworkError) { client.boom }
    assert_equal 'Errno::ECONNREFUSED', error.original_class,
                 'the errno is the real cause and the only thing that proves nothing was sent'
  end

  test 'a bare adapter connection refusal during placement stays retryable' do
    @bot.exchange.stubs(:market_buy).raises(
      Client::TransientNetworkError.new('Faraday::ConnectionFailed: connection refused: 18.132.181.159:8100',
                                        original_class: 'Net::HTTP::Persistent::Error')
    )

    assert_raises(Client::TransientNetworkError) do
      @bot.market_buy(ticker: @ticker, amount: 10, amount_type: :quote)
    end
  end

  # net-http raises Net::OpenTimeout for a TCP connect timeout with Errno::ETIMEDOUT as its cause.
  # The phase-specific class must win over the generic errno: ETIMEDOUT alone is ambiguous (it can
  # equally come from a read), while Net::OpenTimeout proves the connection was never established.
  # Preferring the errno here would turn a provably pre-transmission failure into a skipped
  # interval — the very regression this classification exists to avoid.
  test 'Client#with_rescue prefers Net::OpenTimeout over its errno cause' do
    client = Class.new(Client) do
      def boom
        with_rescue do
          raise Errno::ETIMEDOUT, 'Connection timed out'
        rescue StandardError
          raise Net::OpenTimeout, 'execution expired'
        end
      end
    end.new

    error = assert_raises(Client::TransientNetworkError) { client.boom }
    assert_equal 'Net::OpenTimeout', error.original_class
  end

  test 'a connect timeout carrying an errno cause stays retryable during placement' do
    @bot.exchange.stubs(:market_buy).raises(
      Client::TransientNetworkError.new('Net::OpenTimeout: execution expired',
                                        original_class: 'Net::OpenTimeout')
    )

    assert_raises(Client::TransientNetworkError) do
      @bot.market_buy(ticker: @ticker, amount: 10, amount_type: :quote)
    end
  end

  test 'Client#with_rescue records the underlying cause through Faraday wrapping' do
    client = Class.new(Client) do
      def boom
        with_rescue { raise Faraday::ConnectionFailed, Errno::ECONNREFUSED.new('refused') }
      end
    end.new

    error = assert_raises(Client::TransientNetworkError) { client.boom }
    assert_equal 'Errno::ECONNREFUSED', error.original_class,
                 'the wrapped cause is what tells us whether the request was ever transmitted'
  end

  # --- reads and cancels stay retryable ------------------------------------------------------
  # Scoping matters as much as the conversion. A balance read or an order poll is idempotent, so
  # converting those too would throw away the retry behaviour that keeps bots alive through the
  # documented AWS tinyproxy blips (see Exchange::NETWORK_TRANSIENT_PATTERNS).

  test 'get_balances still raises TransientNetworkError' do
    @bot.exchange.stubs(:get_balances).raises(@timeout)

    assert_raises(Client::TransientNetworkError) { @bot.get_balances }
  end

  test 'get_balance still raises TransientNetworkError' do
    @bot.exchange.stubs(:get_balance).raises(@timeout)

    assert_raises(Client::TransientNetworkError) { @bot.get_balance(asset_id: @ticker.base_asset_id) }
  end

  test 'get_order still raises TransientNetworkError' do
    @bot.exchange.stubs(:get_order).raises(@timeout)

    assert_raises(Client::TransientNetworkError) { @bot.get_order(order_id: 'abc') }
  end

  test 'get_orders still raises TransientNetworkError' do
    @bot.exchange.stubs(:get_orders).raises(@timeout)

    assert_raises(Client::TransientNetworkError) { @bot.get_orders(order_ids: %w[abc def]) }
  end

  # Cancelling twice is harmless (the second cancel fails against an already-gone order), so a
  # cancel keeps its retry. Deliberately unchanged by this fix.
  test 'cancel_order still raises TransientNetworkError' do
    @bot.exchange.stubs(:cancel_order).raises(@timeout)

    assert_raises(Client::TransientNetworkError) { @bot.cancel_order(order_id: 'abc') }
  end

  # --- everything else is untouched ----------------------------------------------------------

  test 'a rate limit on placement is not converted' do
    @bot.exchange.stubs(:market_buy).raises(Client::RateLimitedError.new('slow down'))

    assert_raises(Client::RateLimitedError) do
      @bot.market_buy(ticker: @ticker, amount: 10, amount_type: :quote)
    end
  end

  test 'a successful placement result passes through unchanged' do
    @bot.exchange.stubs(:market_buy).returns(Result::Success.new(order_id: 'X1'))

    result = @bot.market_buy(ticker: @ticker, amount: 10, amount_type: :quote)

    assert result.success?
    assert_equal 'X1', result.data[:order_id]
  end

  test 'a business failure result passes through unchanged' do
    @bot.exchange.stubs(:market_buy)
        .returns(Result::Failure.new('Account has insufficient balance for requested action.'))

    result = @bot.market_buy(ticker: @ticker, amount: 10, amount_type: :quote)

    assert result.failure?
    assert_equal ['Account has insufficient balance for requested action.'], result.errors
  end

  test 'a non-network StandardError from placement is not converted' do
    @bot.exchange.stubs(:market_buy).raises(ArgumentError.new('bad size'))

    assert_raises(ArgumentError) do
      @bot.market_buy(ticker: @ticker, amount: 10, amount_type: :quote)
    end
  end
end
