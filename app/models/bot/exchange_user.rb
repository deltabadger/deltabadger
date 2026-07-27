module Bot::ExchangeUser
  extend ActiveSupport::Concern

  # Network failures that provably occurred BEFORE the request was transmitted: the connection was
  # never established, so nothing reached the exchange and a retry cannot place a second order.
  # These keep the ordinary retry — converting them would cost a bot a full interval (up to a
  # month) every time the AWS exchange proxy is down, a documented recurring outage.
  #
  # Everything else — read timeouts, connection RESETS, and anything of unknown provenance — is
  # ambiguous and must not be retried. The asymmetry is deliberate: a wrongly-retried placement
  # spends the user's money twice, a wrongly-skipped one costs one tick.
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
  # Client.specific_cause_name prefers the phase-naming Net:: class over its errno cause.
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
  ].freeze

  def get_balance(asset_id:)
    with_api_key do
      exchange.get_balance(asset_id: asset_id)
    end
  end

  def get_balances(asset_ids: nil)
    with_api_key do
      exchange.get_balances(asset_ids: asset_ids)
    end
  end

  def get_order(order_id:)
    with_api_key do
      exchange.get_order(order_id: order_id)
    end
  end

  def get_orders(order_ids:)
    with_api_key do
      exchange.get_orders(order_ids: order_ids)
    end
  end

  def cancel_order(order_id:)
    with_api_key do
      exchange.cancel_order(order_id: order_id)
    end
  end

  # The four methods below PLACE ORDERS and are therefore wrapped in with_placement_guard.
  # Everything above only reads or cancels, and deliberately keeps its retry.

  def market_buy(ticker:, amount:, amount_type:)
    with_placement_guard do
      with_api_key do
        exchange.market_buy(ticker: ticker, amount: amount, amount_type: amount_type)
      end
    end
  end

  def market_sell(ticker:, amount:, amount_type:)
    with_placement_guard do
      with_api_key do
        exchange.market_sell(ticker: ticker, amount: amount, amount_type: amount_type)
      end
    end
  end

  def limit_buy(ticker:, amount:, amount_type:, price:)
    with_placement_guard do
      with_api_key do
        exchange.limit_buy(ticker: ticker, amount: amount, amount_type: amount_type, price: price)
      end
    end
  end

  def limit_sell(ticker:, amount:, amount_type:, price:)
    with_placement_guard do
      with_api_key do
        exchange.limit_sell(ticker: ticker, amount: amount, amount_type: amount_type, price: price)
      end
    end
  end

  private

  # A network error raised while placing an order leaves the outcome UNKNOWN — the exchange may
  # have accepted it before the response timed out — and placement has no idempotency key. Left as
  # a TransientNetworkError it reaches Bot::ActionJob's `retry_on`, which replays the job and
  # places the order a second time. Latent rather than observed: a 2026-07-27 fleet scan found 24
  # close-together duplicates, but they traced to user-initiated restarts, not to this path.
  #
  # Reads (get_balance/get_balances/get_order/get_orders) and cancels are idempotent, so they are
  # NOT wrapped: they keep the retry that carries bots through the AWS exchange-proxy blips
  # catalogued in Exchange::NETWORK_TRANSIENT_PATTERNS.
  #
  # KNOWN LIMITATION — the boundary is the whole placement method, not the submit request alone.
  # Where an exchange makes NETWORK CALLS BEFORE submitting, a failure in that preflight is
  # provably pre-transmission yet is still treated as ambiguous, costing one tick. In practice this
  # reaches only Exchanges::Ibkr, which resolves a conid and an account id first: honeymaker-backed
  # exchanges never raise here at all (their client returns a Result::Failure for network errors,
  # which flows through Exchange#placement_transient_error? instead), and Alpaca's placement calls
  # create_order directly with no preflight. Narrowing the boundary to the submit call itself needs
  # a per-client change, and 13 of those clients live in the honeymaker gem — so it is deliberately
  # left as a follow-up. The failure mode is a skipped interval, never a duplicate buy.
  def with_placement_guard
    yield
  rescue Client::TransientNetworkError => e
    raise if pre_transmission?(e)

    raise Client::AmbiguousPlacementError, e.message
  end

  def pre_transmission?(error)
    return true if error.original_class.in?(PRE_TRANSMISSION_ERRORS)

    # The net_http_persistent adapter can surface a dead proxy as a BARE
    # "connection refused: HOST:PORT" with no errno anywhere in the cause chain — the same quirk
    # Exchange::NETWORK_TRANSIENT_PATTERNS documents having been bitten by. A refusal means the
    # connection was never accepted, so nothing was transmitted.
    error.message.to_s.match?(/connection refused/i)
  end
end
