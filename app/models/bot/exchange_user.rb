module Bot::ExchangeUser
  extend ActiveSupport::Concern

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
  # catalogued in Client::NETWORK_TRANSIENT_PATTERNS.
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
  #
  # Network failures that provably occurred BEFORE the request was transmitted (Client.pre_transmission?:
  # the connection was never established, so nothing reached the exchange and a retry cannot place a
  # second order) keep the ordinary retry — converting them would cost a bot a full interval (up to a
  # month) every time an exchange proxy is down. Everything else — read timeouts, connection RESETS,
  # and anything of unknown provenance — is ambiguous and must not be retried. The asymmetry is
  # deliberate: a wrongly-retried placement spends the user's money twice, a wrongly-skipped one costs
  # one tick.
  def with_placement_guard
    yield
  rescue Client::TransientNetworkError => e
    raise if Client.pre_transmission?(e.original_class, e.message)

    raise Client::AmbiguousPlacementError, e.message
  end
end
