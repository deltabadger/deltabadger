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

  TRANSIENT_NETWORK_ERRORS = [
    Net::OpenTimeout,
    Faraday::ConnectionFailed,
    Faraday::TimeoutError
  ].freeze

  OPTIONS = {
    request: {
      open_timeout: 5,   # seconds to wait for the connection to open
      read_timeout: 30,  # seconds to wait for one block to be read
      write_timeout: 10  # seconds to wait for one block to be written
    }
  }.freeze

  # Build the TransientNetworkError for a raw network exception, preserving provenance.
  # SHARED because Clients::Alpaca and Clients::Ibkr override #with_rescue: an override that
  # rebuilt this by hand would drop original_class, and a nil original_class is treated as
  # "the request may have landed" by Bot::ExchangeUser#with_placement_guard — silently turning a
  # definitive connect timeout into a skipped trading interval.
  def self.transient_network_error(error)
    TransientNetworkError.new("#{error.class}: #{error.message}", original_class: specific_cause_name(error))
  end

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
  PREFERRED_CAUSE_PATTERNS = [
    /\ANet::(Open|Read)Timeout\z/,
    /\AErrno::/,
    /\A(SocketError|Socket::Resolution|Resolv::)/
  ].freeze

  # Walk BOTH link types — Faraday's #wrapped_exception and Ruby's #cause — because adapters nest
  # them differently, and return the most specific network cause found. Falls back to the outermost
  # class when nothing more specific exists (e.g. a bare Faraday::TimeoutError), which is correctly
  # treated as ambiguous by Bot::ExchangeUser#with_placement_guard.
  def self.specific_cause_name(error)
    names = []
    queue = [error]
    10.times do
      node = queue.shift
      break if node.nil?
      next if names.include?(node.class.name)

      names << node.class.name
      queue << node.wrapped_exception if node.respond_to?(:wrapped_exception) && node.wrapped_exception
      queue << node.cause if node.cause
    end

    PREFERRED_CAUSE_PATTERNS.each do |pattern|
      match = names.find { |name| name.match?(pattern) }
      return match if match
    end
    names.first
  end

  def with_rescue
    yield
  rescue *TRANSIENT_NETWORK_ERRORS => e
    raise Client.transient_network_error(e)
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
