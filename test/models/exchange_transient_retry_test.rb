require 'test_helper'

class ExchangeTransientRetryTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:binance_exchange)
  end

  # base_delay: 0 keeps the loop instant without stubbing the private `sleep`.

  test 'returns immediately on success without retrying' do
    calls = 0
    result = @exchange.with_transient_retry(base_delay: 0) do
      calls += 1
      Result::Success.new(:ok)
    end
    assert result.success?
    assert_equal 1, calls
  end

  test 'retries a transient failure then succeeds' do
    seq = [Result::Failure.new('Timestamp for this request is outside of the recvWindow'),
           Result::Success.new(:ok)]
    result = @exchange.with_transient_retry(base_delay: 0) { seq.shift }
    assert result.success?
    assert_empty seq
  end

  test 'gives up after attempts and returns the last transient failure' do
    calls = 0
    result = @exchange.with_transient_retry(attempts: 3, base_delay: 0) do
      calls += 1
      Result::Failure.new('Timestamp for this request is outside of the recvWindow')
    end
    assert result.failure?
    assert_equal 3, calls
  end

  test 'does NOT retry a non-transient failure' do
    calls = 0
    result = @exchange.with_transient_retry(base_delay: 0) do
      calls += 1
      Result::Failure.new('Account has insufficient balance for requested action.')
    end
    assert result.failure?
    assert_equal 1, calls
  end

  # placement_transient_error? is DELIBERATELY narrower than transient_error?:
  # it matches ONLY the -1021/timestamp rejection (order never placed → safe), and NEVER
  # network timeouts (the order may have reached the book → double-order risk), and NEVER
  # other exchanges' ambiguous :transient strings.
  test 'placement_transient_error? matches the -1021 timestamp rejection' do
    assert @exchange.placement_transient_error?(['Timestamp for this request is outside of the recvWindow.'])
    assert @exchange.placement_transient_error?(["Timestamp for this request was 1000ms ahead of the server's time."])
  end

  test 'placement_transient_error? NEVER matches network timeouts (double-order safety)' do
    refute @exchange.placement_transient_error?(['Net::ReadTimeout with #<TCPSocket:(closed)>'])
    refute @exchange.placement_transient_error?(['Faraday::TimeoutError: read timed out'])
    refute @exchange.placement_transient_error?(['Faraday::ConnectionFailed: connection refused'])
    refute @exchange.placement_transient_error?(['execution expired'])
    refute @exchange.placement_transient_error?(['Errno::ECONNRESET: Connection reset by peer'])
  end

  test 'placement_transient_error? does NOT match business/auth errors' do
    refute @exchange.placement_transient_error?(['Account has insufficient balance for requested action.'])
    refute @exchange.placement_transient_error?(['Filter failure: MIN_NOTIONAL'])
  end

  # CRITICAL (Codex rounds 1+2): Kraken HAS known_errors[:transient] = ['EGeneral:Internal error',
  # 'EAPI:Invalid nonce', …]. Those are HTTP-200 transient READ failures, NOT guaranteed pre-trade
  # rejections — a Kraken AddOrder 'Internal error' could mean the order reached the engine. The
  # placement predicate keys off the dedicated PLACEMENT_SAFE_TRANSIENT_ERRORS allowlist (NOT
  # known_errors[:transient]), so Kraken's OWN ambiguous strings must be placement-FALSE while still
  # being READ-transient.
  test 'placement_transient_error? is false for ambiguous Kraken transient strings (NOT pre-trade-safe)' do
    kraken = create(:kraken_exchange)
    assert kraken.transient_error?(['EGeneral:Internal error']), 'sanity: Kraken keeps EGeneral:Internal error as a READ transient'
    refute kraken.placement_transient_error?(['EGeneral:Internal error'])
    refute kraken.placement_transient_error?(['EAPI:Invalid nonce'])
  end
end
