require 'net/http'

class Client
  class TransientNetworkError < StandardError
    # The most specific underlying error class, unwrapped from Faraday where it wraps one
    # (Faraday::ConnectionFailed carries Errno::ECONNREFUSED, Errno::ECONNRESET, SocketError, …).
    # This is what lets a caller tell a failure that provably happened BEFORE the request was
    # transmitted (nothing reached the exchange, safe to retry) from one where the request may
    # already have landed. See Bot::ExchangeUser#with_placement_guard.
    #
    # nil when the error was raised without provenance — callers must treat nil as "unknown",
    # i.e. assume the request may have landed.
    attr_reader :original_class

    def initialize(message = nil, original_class: nil)
      super(message)
      @original_class = original_class
    end
  end

  # An exchange rate-limit / throttle response (e.g. Kraken HTTP-200 "EAPI:Rate limit
  # exceeded"). Distinct from TransientNetworkError so it can retry on its own, longer
  # escalating wait — retrying a rate limit too soon re-trips the decaying counter.
  class RateLimitedError < StandardError; end
  # A network failure during ORDER PLACEMENT, where the outcome is genuinely UNKNOWN: the
  # exchange may have accepted the order before the response timed out. Placement carries no
  # idempotency key, so a replay places a SECOND order.
  #
  # MUST NOT subclass TransientNetworkError. Bot::ActionJob declares
  # `retry_on Client::TransientNetworkError` (added 2026-05-28, 99a4558f8), retry_on matches
  # subclasses, and that retry is exactly the replay this class exists to prevent.
  #
  # LATENT, NOT OBSERVED. A fleet scan on 2026-07-27 found 24 close-together duplicate placements,
  # but forensics attributed them to user-initiated restarts (a start places immediately), not to
  # this path — and 14 of them predate retry_on existing at all. The guard stands because the
  # replay is reachable in code and spends a user's money twice when it fires, not because it has
  # been seen firing.
  #
  # This is the RAISE-path sibling of Exchange::PLACEMENT_SAFE_TRANSIENT_ERRORS, which already
  # encodes the same reasoning for placement failures that arrive as a Result::Failure.
  class AmbiguousPlacementError < StandardError; end

  # Rescued as a TRANSPORT failure: the request got no HTTP answer. Faraday::SSLError is a transport
  # failure too — without it here a TLS error fell through to the generic Faraday::Error branch and
  # left without its exception chain.
  TRANSIENT_NETWORK_ERRORS = [
    Net::OpenTimeout,
    Faraday::ConnectionFailed,
    Faraday::TimeoutError,
    Faraday::SSLError
  ].freeze

  OPTIONS = {
    request: {
      open_timeout: 5,   # seconds to wait for the connection to open
      read_timeout: 30,  # seconds to wait for one block to be read
      write_timeout: 10  # seconds to wait for one block to be written
    }
  }.freeze

  # Exchange-agnostic network failures that are ALWAYS retryable — any exchange can hit these through
  # an HTTP proxy or the network (proxy latency spikes surfaced these as terminal order-fetch
  # errors). Matched as narrow substrings of the error string the exchange returns (no broad "Timeout"/
  # "TCPSocket" — those risk false positives on business/config messages).
  #
  # This is the TEXT rule: what decides when a failure carries nothing more specific than a wrapper
  # class (see .transient_cause?), or no chain at all.
  NETWORK_TRANSIENT_PATTERNS = [
    'Net::ReadTimeout',
    'Net::OpenTimeout',
    'Faraday::TimeoutError',
    'Faraday::ConnectionFailed',
    'execution expired',
    'Connection reset',
    'Errno::ECONNRESET',
    # A dead exchange proxy. The net_http_persistent adapter reports this as a BARE
    # "connection refused: HOST:PORT" (no 'Faraday::ConnectionFailed' prefix), so the class-name
    # pattern above never matched it and every proxied bot failed loudly. Both cases are listed:
    # net_http_persistent lowercases it, Errno::ECONNREFUSED does not.
    'connection refused',
    'Connection refused',
    'Errno::ECONNREFUSED',
    # A persistent connection the venue had already closed, as honeymaker's text reports it, and the
    # same thing over TLS. Listed for failures that carry no exception chain.
    'end of file reached',
    'unexpected eof while reading'
  ].freeze

  # Classes that only say "the transport failed", not how. A chain whose most specific cause is one of
  # these has no provenance worth trusting, so the text decides instead.
  WRAPPER_CAUSES = %w[
    Faraday::ConnectionFailed
    Faraday::TimeoutError
    Faraday::SSLError
    Net::HTTP::Persistent::Error
  ].freeze

  # Specific causes a retry cannot fix, so a read that hits one is NOT retried: a proxy refusing
  # CONNECT (a revoked or filtered proxy credential, which retried as transient would only ever show
  # up as quietly skipped intervals) and a local permission or firewall refusal. TLS failures other
  # than a dropped connection are handled in transient_cause? itself.
  #
  # This is a DENYLIST on purpose — the opposite of PRE_TRANSMISSION_ERRORS — because for a READ the
  # cost asymmetry runs the other way: retrying something that cannot heal costs a few bounded
  # attempts, while failing something that would have healed costs a whole tick or a day's sync. So
  # every other transport cause is retried, name resolution included: a hostname can fail to resolve
  # for a moment (a resolver hiccup, a service being redeployed behind a network alias), and each
  # caller's retry is bounded.
  PERSISTENT_CAUSES = %w[
    Net::HTTPClientException
    Errno::EACCES
    Errno::EPERM
  ].freeze

  # Failures that PROVE nothing reached the exchange: the connection was never established, so a
  # retry cannot place a second order.
  #
  # This is an ALLOWLIST, and it is deliberately incomplete. A denylist ("everything except the
  # errnos that can happen mid-flight") was tried and rejected: it fails toward RETRYING an
  # unrecognised error, and something like Errno::ECONNABORTED can fire on an established socket
  # after the request bytes went out. The cost asymmetry decides it — an errno missing from this
  # list costs one skipped tick, an errno wrongly on it spends the user's money twice. So unknown
  # provenance must mean "assume it may have landed".
  #
  # Socket::ResolutionError is Ruby 3.3+'s DNS failure and a SUBCLASS of SocketError, so an exact
  # name match needs both spellings. ETIMEDOUT is deliberately ABSENT: a bare socket timeout is
  # ambiguous, and a genuine CONNECT timeout arrives as Net::OpenTimeout because
  # .most_specific_cause prefers the phase-naming Net:: class over its errno cause.
  # Net::HTTPClientException is an HTTP proxy refusing the CONNECT: the tunnel to the venue was never
  # opened.
  PRE_TRANSMISSION_ERRORS = %w[
    Net::OpenTimeout
    SocketError
    Socket::ResolutionError
    Resolv::ResolvError
    Errno::ECONNREFUSED
    Errno::EHOSTUNREACH
    Errno::EHOSTDOWN
    Errno::ENETUNREACH
    Errno::ENETDOWN
    Errno::EADDRNOTAVAIL
    Net::HTTPClientException
  ].freeze

  # Preference order, most PHASE-INFORMATIVE first. What matters to a caller is not which errno
  # fired but WHEN: was the request ever transmitted?
  #
  # 1. Net::OpenTimeout / Net::ReadTimeout name the phase outright, and each carries a generic
  #    errno as its cause (a connect timeout is Net::OpenTimeout caused by Errno::ETIMEDOUT).
  #    They must win — ETIMEDOUT alone is ambiguous because a read can raise it too, so preferring
  #    the errno would turn a provably pre-transmission failure into a skipped interval.
  # 2. Then concrete errnos, which do carry phase information (ECONNREFUSED = never accepted,
  #    ECONNRESET = possibly mid-flight). This tier is why the list is walked at all: the
  #    net_http_persistent adapter buries ECONNREFUSED under its own Net::HTTP::Persistent::Error,
  #    which a naive "first Net:: class" rule would report instead.
  # 3. Then name-resolution failures, which are always pre-transmission.
  # 4. Then what a wrapper hides that has no errno: a connection closed under us, a TLS failure, a
  #    proxy refusing CONNECT. Last, so none of them can outrank the tiers above.
  PREFERRED_CAUSE_PATTERNS = [
    /\ANet::(Open|Read)Timeout\z/,
    /\AErrno::/,
    /\A(SocketError|Socket::Resolution|Resolv::)/,
    /\A(EOFError|OpenSSL::SSL::SSLError|Net::HTTPClientException)\z/
  ].freeze

  # The ONE place a transport failure is classified for the app's own clients. Clients::Alpaca and
  # Clients::Ibkr override #with_rescue and call this too: an override that rebuilt the error by hand
  # would drop original_class, and a nil original_class is treated as "the request may have landed"
  # by Bot::ExchangeUser#with_placement_guard — silently turning a definitive connect timeout into a
  # skipped trading interval.
  #
  # A failure a retry can fix is RAISED, for the callers' retry_on. Anything else is RETURNED with its
  # exception chain, the same shape honeymaker reports, so Exchange#transient_failure? and
  # Exchange#ambiguous_placement_error? read both families alike.
  def self.network_failure(error)
    message = "#{error.class}: #{error.message}"
    chain = error_chain(error)
    cause = most_specific_cause(chain)
    raise TransientNetworkError.new(message, original_class: cause) if transient_cause?(cause, message)

    Result::Failure.new(message, data: { status: nil, error_chain: chain })
  end

  # Walk BOTH link types — Faraday's #wrapped_exception and Ruby's #cause — because adapters nest
  # them differently. Class names, outermost first, each once.
  def self.error_chain(error)
    names = []
    seen = {}.compare_by_identity # exceptions, not classes: adapters re-wrap in the same class
    queue = [error]
    10.times do
      node = queue.shift
      break if node.nil?
      next if seen.key?(node)

      seen[node] = true
      names << node.class.name unless names.include?(node.class.name)
      queue << node.wrapped_exception if node.respond_to?(:wrapped_exception) && node.wrapped_exception
      queue << node.cause if node.cause
    end
    names
  end

  # The most specific network cause in a chain. Falls back to the outermost class when nothing more
  # specific exists (e.g. a bare Faraday::TimeoutError), which is correctly treated as ambiguous by
  # Bot::ExchangeUser#with_placement_guard.
  def self.most_specific_cause(names)
    PREFERRED_CAUSE_PATTERNS.each do |pattern|
      match = names.find { |name| name.match?(pattern) }
      return match if match
    end
    names.first
  end

  # .most_specific_cause for an exception rather than a chain of class names.
  def self.specific_cause_name(error)
    most_specific_cause(error_chain(error))
  end

  # Would a retry fix it? A specific cause decides alone. A wrapper names no cause, so the text
  # decides — which keeps the retries a bare "Faraday::TimeoutError: execution expired" and a bare
  # "connection refused: HOST:PORT" have always had.
  #
  # A TLS error is retried only when the peer closed the connection under it ("unexpected eof"); a
  # certificate or protocol failure fails the same way every time.
  def self.transient_cause?(cause, message)
    text = message.to_s
    return NETWORK_TRANSIENT_PATTERNS.any? { |pattern| text.include?(pattern) } if cause.nil? || cause.in?(WRAPPER_CAUSES)
    return text.match?(/unexpected eof/i) if cause == 'OpenSSL::SSL::SSLError'

    !cause.in?(PERSISTENT_CAUSES)
  end

  # Did the request provably never leave? Only then may a failed placement be retried or written down
  # as failed — see Bot::ExchangeUser#with_placement_guard and Exchange#ambiguous_placement_error?.
  #
  # The net_http_persistent adapter can surface a dead proxy as a BARE "connection refused: HOST:PORT"
  # with no errno anywhere in the cause chain — the same quirk NETWORK_TRANSIENT_PATTERNS documents
  # having been bitten by. A refusal means the connection was never accepted, so nothing was
  # transmitted.
  def self.pre_transmission?(cause, message)
    cause.in?(PRE_TRANSMISSION_ERRORS) || message.to_s.match?(/connection refused/i)
  end

  def with_rescue
    yield
  rescue *TRANSIENT_NETWORK_ERRORS => e
    Client.network_failure(e)
  rescue Faraday::ParsingError => e
    # The venue answered and we could not read the answer. For a placement that is an accepted
    # order with no acknowledgement, so the provenance travels with the failure — see
    # Exchange#ambiguous_placement_error?.
    Result::Failure.new("Unreadable response (HTTP #{e.response_status || 'error'})",
                        data: { status: e.response_status, unreadable: true })
  rescue Faraday::Error => e
    body = e.response_body.presence
    error_message = if body&.match?(/<\s*html/i)
                      "HTTP #{e.response_status || 'error'}"
                    else
                      body || e.message.presence || 'Unknown API error'
                    end
    Result::Failure.new(error_message, data: { status: e.response_status })
  rescue StandardError => e
    Result::Failure.new(e.message.presence || 'Unknown error')
  end
end
