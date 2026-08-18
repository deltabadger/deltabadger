class Exchange < ApplicationRecord
  # Backstop for windowed candle pagination loops: well above any real fetch (the ~20y ATH
  # lookback over daily candles is only a handful of windows) so a misbehaving API or an
  # advance bug can never spin forever.
  MAX_CANDLE_PAGES = 1000
  # "We do not accept these credentials", in the one status every venue agrees on. It SURFACES a
  # credential failure (#invalid_key_error?) but never condemns a stored key on its own
  # (#condemning_invalid_key_error?). Exchanges::Ibkr opts out entirely via #ambiguous_unauthorized?.
  UNAUTHORIZED_STATUS = 401

  STABLE_TYPES = %w[Exchanges::Binance Exchanges::BinanceUs Exchanges::Coinbase Exchanges::Kraken].freeze

  # Venues that have ceased to exist. The STI class stays behind (a stub — see
  # Exchanges::Bitmart) so an install still holding bots and trade history keeps loading and can
  # move those bots elsewhere, but nothing may trade, connect a key, or start against them again.
  # Listed as types rather than an `available` flag so the guards stay queryable in SQL and one
  # constant drives them all.
  RETIRED_TYPES = %w[Exchanges::Bitmart].freeze

  # Stock brokers (as opposed to crypto exchanges). Stock bots route to one of these; the
  # rest of the app (tax report, bot tile, broker picker) keys off this instead of hardcoding
  # Exchanges::Alpaca as the sole stock venue.
  STOCK_TYPES = %w[Exchanges::Alpaca Exchanges::Ibkr].freeze

  # Exchange-agnostic network failures that are ALWAYS retryable — any exchange can hit these through
  # the HTTP proxy / network (the UK proxy's latency spikes surfaced these as terminal order-fetch
  # errors). Matched as narrow substrings of the error string the exchange returns (no broad "Timeout"/
  # "TCPSocket" — those risk false positives on business/config messages).
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
    'Errno::ECONNREFUSED'
  ].freeze

  # Placement-SAFE transient errors: a strict allowlist of strings that each guarantee the exchange
  # rejected the order BEFORE it reached the matching engine (no order placed → re-placing with a
  # fresh timestamp is safe). Currently only the Binance-family -1021/timestamp rejection qualifies.
  # This is INTENTIONALLY separate from per-exchange known_errors[:transient]: that set can contain
  # AMBIGUOUS strings (e.g. Kraken's 'EGeneral:Internal error', 'EAPI:Invalid nonce') where the order
  # MIGHT have hit the book — those must NEVER be treated as placement-safe. Adding a string here is a
  # double-order-safety decision: only add errors that are provably pre-trade rejections.
  PLACEMENT_SAFE_TRANSIENT_ERRORS = [
    'Timestamp for this request is outside of the recvWindow',
    'Timestamp for this request was' # "…Nms ahead of the server's time."
  ].freeze

  scope :stock_venues, -> { where(type: STOCK_TYPES) }

  has_many :bots
  has_many :api_keys
  has_one :fee_api_key
  has_many :exchange_assets
  has_many :assets, through: :exchange_assets
  has_many :tickers
  has_many :transactions
  has_many :account_transactions

  validates :name, presence: true
  validates :type, uniqueness: true

  scope :available, -> { where(available: true) }
  # Everywhere a user could pick or reach an exchange to trade on. `available` alone is not enough:
  # the flag is data and can be flipped back by a sync, retirement is a property of the class.
  scope :tradeable, -> { available.where.not(type: RETIRED_TYPES) }

  def beta?
    !type.in?(STABLE_TYPES)
  end

  def retired?
    type.in?(RETIRED_TYPES)
  end

  def stock_venue?
    type.in?(STOCK_TYPES)
  end

  # Some exchange adapters have an optional write-side dependency. Read APIs remain useful when
  # that dependency is not part of this installation, so callers gate only order placement.
  def order_placement_available?
    true
  end

  include Synchronizer
  include CandleBuilder

  def symbols
    return Result::Success.new([]) if name.downcase == 'alpaca'

    ExchangeMarket.new(self).all_symbols("#{name.downcase}_all_symbols")
  end

  def name_id
    self.class.name.demodulize.underscore
  end

  # How far back `AccountTransactionSync` may put a ledger window's start, or `nil` for no limit.
  #
  # Declare it ONLY on a venue whose endpoint caps the returned window. Those endpoints measure the
  # cap FROM `startTime`, so a start parked further back than the cap returns a window that ends in
  # the past and the account goes silently blind — nothing new arrives, so the data-derived watermark
  # can never advance to un-blind it. There, clamping to the cap costs no history the endpoint would
  # have returned anyway.
  #
  # On an UNCAPPED venue the same clamp is pure data loss. Alpaca, Kraken, Coinbase and Hyperliquid
  # all paginate a cursor from `start_time` to the present, and there is no recurring ledger sync —
  # a sync only happens when the user opens the tracker. So a clamp here means a buy-and-hold user
  # who visits in January and again in August never fetches the months in between, the watermark then
  # advances past them, and nothing banners. `nil` is the safe default; a cap is the exception.
  def ledger_window = nil

  def coingecko_id
    raise NotImplementedError, "#{self.class.name} must implement coingecko_id"
  end

  def known_errors
    raise NotImplementedError, "#{self.class.name} must implement known_errors"
  end

  def set_client(api_key: nil)
    raise NotImplementedError, "#{self.class.name} must implement set_client"
  end

  def get_tickers_info(force: false)
    raise NotImplementedError, "#{self.class.name} must implement get_tickers_info"
  end

  def get_tickers_prices(force: false, symbols: nil)
    raise NotImplementedError, "#{self.class.name} must implement get_tickers_prices"
  end

  def get_balances(asset_ids: nil)
    raise NotImplementedError, "#{self.class.name} must implement get_balances"
  end

  def get_balance(asset_id:)
    result = get_balances(asset_ids: [asset_id])
    return result if result.failure?

    Result::Success.new(result.data[asset_id])
  end

  # What an account can actually spend, which is not always its settled balance — margin venues
  # let a bot spend against borrowed buying power. Takes an already-fetched balance hash, so
  # asking costs no extra network read. Overridden by the venues where the two differ.
  def spendable_balance(balance, tickers: nil)
    balance[:free]
  end

  def get_last_price(ticker:, force: false)
    raise NotImplementedError, "#{self.class.name} must implement get_last_price"
  end

  # Exchanges may quote the USD price of some assets directly (e.g. Alpaca
  # knows stock prices). Override in subclasses. Returns a Result wrapping
  # { external_id => usd_price } for only the assets this exchange can price;
  # AccountBalance::Sync falls back to MarketData for anything missing.
  def get_usd_prices(assets:)
    Result::Success.new({})
  end

  def get_bid_price(ticker:, force: false)
    raise NotImplementedError, "#{self.class.name} must implement get_bid_price"
  end

  def get_ask_price(ticker:, force: false)
    raise NotImplementedError, "#{self.class.name} must implement get_ask_price"
  end

  def get_candles(ticker:, start_at:, timeframe:)
    raise NotImplementedError, "#{self.class.name} must implement get_candles"
  end

  def market_buy(ticker:, amount:, amount_type:)
    raise NotImplementedError, "#{self.class.name} must implement market_buy"
  end

  def market_sell(ticker:, amount:, amount_type:)
    raise NotImplementedError, "#{self.class.name} must implement market_sell"
  end

  def limit_buy(ticker:, amount:, amount_type:, price:)
    raise NotImplementedError, "#{self.class.name} must implement limit_buy"
  end

  def limit_sell(ticker:, amount:, amount_type:, price:)
    raise NotImplementedError, "#{self.class.name} must implement limit_sell"
  end

  # Format a price to the exchange's tick rule before it is sent to the API.
  # Default: fixed decimal places. Exchanges with significant-figure tick rules
  # (e.g. Hyperliquid) override this.
  def adjusted_price(ticker:, price:, method: :floor)
    price.public_send(method, ticker.price_decimals)
  end

  def get_order(order_id:)
    raise NotImplementedError, "#{self.class.name} must implement get_order"
  end

  def get_orders(order_ids:)
    raise NotImplementedError, "#{self.class.name} must implement get_orders"
  end

  # True only for exchanges whose get_orders exhausts every fill source (e.g. Kraken:
  # QueryOrders + TradesHistory). When false, a missing order might be an undetected fill,
  # so Bot::FetchAndUpdateOpenOrdersJob keeps raising loudly instead of silently proceeding.
  def authoritative_missing_orders?
    false
  end

  def cancel_order(order_id:)
    raise NotImplementedError, "#{self.class.name} must implement cancel_order"
  end

  # Translate a raw exchange error string into a user-friendly localized
  # message via Honeymaker's per-exchange classifier. Falls back to the raw
  # message when the exchange or pattern is unknown, so unmatched errors
  # still surface verbatim instead of disappearing.
  def humanize_error(message)
    return nil if message.nil?

    klass = Honeymaker::EXCHANGES[name_id]
    return message unless klass

    classification = klass.new.classify_error(message)
    return message unless classification

    code = classification[:code]
    params = classification.except(:code).merge(exchange: name)
    I18n.t("errors.exchange.#{code}", **params)
  end

  # Heuristic: does the given errors array look like an invalid-key / auth error?
  # Used by sync jobs to decide whether to flip an API key's status to :incorrect
  # when a live call (get_balances, get_ledger, etc.) fails.
  def invalid_key_error?(errors, status: nil)
    # HTTP 401 is the one vocabulary every venue shares for "these credentials are not accepted", and
    # it is the only signal some venues give: Coinbase carries no usable message string at all, so
    # without it a rejected Coinbase key produces a domain-shaped lie downstream and nothing else.
    # Honeymaker::Client#with_rescue and Clients::Alpaca both attach it; venues that reject over
    # HTTP 200 (Kraken's EAPI:Invalid key) still need the strings below, so this is an addition.
    #
    # SURFACING ONLY. A bare 401 is deliberately NOT enough to condemn a stored key — see
    # #condemning_invalid_key_error?. Coinbase signs each request with a two-minute JWT built from
    # local time, so clock skew rejects a perfectly good key; our own authenticated exchange proxy
    # answers a wrong password with 401 as well. Saying "the venue rejected our credentials" in a bot
    # error is reversible and self-correcting when either of those is the real cause. Flipping the
    # key to :incorrect is not: it drops the key from every :correct-scoped sync until the user
    # pastes new credentials that were never the problem.
    return true if status == UNAUTHORIZED_STATUS && !ambiguous_unauthorized?

    invalid_messages = (known_errors[:invalid_key] || []).map(&:to_s)
    return false if invalid_messages.empty?

    Array(errors).any? do |err|
      msg = err.to_s
      invalid_messages.any? { |m| msg.include?(m) }
    end
  end

  # Raise on a credential rejection, wherever a failed call would otherwise be flattened into a
  # benign answer — "no price", "market state unknown", "condition not met". Those flattenings are
  # right for a pair with no liquidity, a clock blip or a price that simply has not moved; for a
  # rejected key they are a lie that hides the one thing the user has to act on, and they hid it for
  # six weeks. Single phrase, single place: the venue's own text can be as bare as "HTTP 401", so
  # the exchange is named here. The localized, actionable copy stays the tracker's sync-key banner,
  # which the same classification drives.
  def raise_on_invalid_key!(result)
    return if result.success?
    return unless invalid_key_error?(result.errors, status: http_status(result))

    raise "#{name} rejected the API key: #{Array(result.errors).to_sentence}"
  end

  # The venue's HTTP status when the client attached one (both honeymaker and the app's own clients
  # carry it as data: { status: }), nil when the failure never had one — a Kraken HTTP-200 rejection
  # or an error we raised ourselves.
  def http_status(result)
    result.data[:status] if result.data.is_a?(Hash)
  end

  # Enough to CONDEMN the stored key — persistent, and only the user can undo it. Deliberately
  # narrower than #invalid_key_error?: the venue's own words, never a bare status. Every 401 we
  # cannot attribute (Coinbase's JWT clock skew, a proxy rejecting its own credential, a WAF) would
  # otherwise strand a user whose key is fine, and "replace your API key" is advice they cannot act
  # on. Alpaca's two observed rejection bodies are listed as strings for exactly this reason.
  def condemning_invalid_key_error?(errors)
    invalid_key_error?(errors)
  end

  # Does a 401 from this venue mean anything OTHER than "your key is no longer valid"? Only where
  # it does may a venue refuse the status rule — see Exchanges::Ibkr, whose competing-login and
  # not-yet-activated sessions 401 exactly like a revoked key.
  def ambiguous_unauthorized?
    false
  end

  # Heuristic: do the given errors say the credentials are fine but lack a SCOPE the call needed
  # (e.g. Kraken's "EGeneral:Permission denied" on Ledgers when the key has no Query Ledger
  # Entries)? Deliberately separate from invalid_key_error?: the key is valid and may be trading
  # happily, so condemning it — which removes it from every :correct-scoped sync — is both too
  # harsh and useless, since re-pasting the same credentials cannot add a permission.
  #
  # Only venues that emit an UNAMBIGUOUS scope error populate :permission_denied. Binance and
  # Bybit's "Invalid API-key, IP, or permissions for action." stays in :invalid_key: the venue
  # conflates revoked key, wrong IP and missing scope, and "replace the key" fits all three.
  def permission_error?(errors)
    permission_messages = (known_errors[:permission_denied] || []).map(&:to_s)
    return false if permission_messages.empty?

    Array(errors).any? do |err|
      msg = err.to_s
      permission_messages.any? { |m| msg.include?(m) }
    end
  end

  # Heuristic: do the given errors look like a transient/retryable exchange API
  # failure (e.g. Kraken's HTTP-200 "EGeneral:Internal error" / "EAPI:Invalid nonce")?
  # Used by the fetch jobs to convert such failures into Client::TransientNetworkError
  # so they flow into the existing retry-with-backoff path instead of failing loudly.
  def transient_error?(errors)
    # Base network patterns apply to EVERY exchange (incl. those with no exchange-specific :transient
    # set, e.g. Binance) — so this must not early-return on an empty known_errors[:transient].
    patterns = NETWORK_TRANSIENT_PATTERNS + (known_errors[:transient] || []).map(&:to_s)

    Array(errors).any? do |err|
      msg = err.to_s
      patterns.any? { |m| msg.include?(m) }
    end
  end

  # Sibling of transient_error?, but DELIBERATELY NARROWER — for the order-PLACEMENT site only.
  # Matches ONLY PLACEMENT_SAFE_TRANSIENT_ERRORS, and NEVER NETWORK_TRANSIENT_PATTERNS nor the broader
  # known_errors[:transient]. Rationale: a -1021 is a definitive pre-trade rejection (the order never
  # reached the matching engine → re-placing with a fresh timestamp is safe), whereas a placement
  # network timeout OR an ambiguous exchange error is indistinguishable from a successful book hit, and
  # placement has no idempotency key → it must NOT be treated as safely transient. Do not widen this.
  #
  # A venue may add its own strings via known_errors[:placement_safe_transient], scoped to itself so
  # one venue's wording can never vouch for another's. The bar is unchanged and deliberately high:
  # the string must guarantee a PRE-TRADE rejection. Signed-timestamp rejections qualify because the
  # timestamp is part of the signed payload — failing it fails authentication, and an unauthenticated
  # request never reaches the matching engine. That is the same proof that admitted Binance's -1021.
  # Note this is opt-in separately from known_errors[:transient]: being retryable on a READ says
  # nothing about being safe to RE-PLACE, so the string must be listed twice, on purpose.
  def placement_transient_error?(errors)
    patterns = PLACEMENT_SAFE_TRANSIENT_ERRORS + (known_errors[:placement_safe_transient] || []).map(&:to_s)
    Array(errors).any? do |err|
      msg = err.to_s
      patterns.any? { |m| msg.include?(m) }
    end
  end

  # Retry an idempotent READ that returns a Result, when it fails with a transient error
  # (network blip / -1021 timestamp). READS ONLY — never wrap order placement or withdrawals,
  # which must stay single-shot. Bots don't use this (they retry at the job level); this is for
  # the rule path, which has no job-level retry.
  def with_transient_retry(attempts: 3, base_delay: 0.5)
    result = yield
    tries = 1
    while tries < attempts && result.respond_to?(:failure?) && result.failure? && transient_error?(result.errors)
      sleep(base_delay * tries)
      result = yield
      tries += 1
    end
    result
  end

  # Sibling of transient_error?: do the given errors look like an exchange rate-limit /
  # throttle response? The fetch jobs convert these into Client::RateLimitedError so they
  # retry on a longer, escalating wait (BotJob::RATE_LIMIT_WAIT) instead of failing loudly.
  def throttled_error?(errors)
    throttle_messages = (known_errors[:throttle] || []).map(&:to_s)
    return false if throttle_messages.empty?

    Array(errors).any? do |err|
      msg = err.to_s
      throttle_messages.any? { |m| msg.include?(m) }
    end
  end

  def market_open?(tickers: nil)
    true
  end

  def next_market_open_at(tickers: nil)
    Time.current
  end

  def supports_withdrawal?
    true
  end

  def list_withdrawal_addresses(asset:)
    nil
  end

  def withdraw(asset:, amount:, address:, network: nil, address_tag: nil)
    raise NotImplementedError, "#{self.class.name} must implement withdraw"
  end

  def get_api_key_validity(api_key:)
    raise NotImplementedError, "#{self.class.name} must implement get_api_key_validity"
  end

  def fetch_withdrawal_fees!
    raise NotImplementedError, "#{self.class.name} must implement fetch_withdrawal_fees!"
  end

  def withdrawal_fee_for(asset:)
    ea = exchange_assets.find_by(asset: asset)
    return nil if ea.nil? || ea.withdrawal_fee.blank?

    BigDecimal(ea.withdrawal_fee)
  end

  def withdrawal_fee_fresh?(asset:)
    ea = exchange_assets.find_by(asset: asset)
    return false if ea.nil? || ea.withdrawal_fee_updated_at.nil?

    ea.withdrawal_fee_updated_at > 24.hours.ago
  end

  def minimum_amount_logic
    raise NotImplementedError, "#{self.class.name} must implement minimum_amount_logic"
  end

  def symbol_from_asset(asset)
    @symbol_from_asset ||= tickers.available.includes(:base_asset, :quote_asset).each_with_object({}) do |t, h|
      h[t.base_asset_id] ||= t.base
      h[t.quote_asset_id] ||= t.quote
    end
    @symbol_from_asset[asset.id]
  end

  def requires_passphrase?
    false
  end

  private

  def client
    @client ||= set_client
  end

  def asset_from_symbol(symbol)
    @asset_from_symbol ||= tickers.available.includes(:base_asset, :quote_asset).each_with_object({}) do |t, h|
      h[t.base] ||= t.base_asset
      h[t.quote] ||= t.quote_asset
    end
    @asset_from_symbol[symbol]
  end

  def parse_order_status(status)
    self.class::ORDER_STATUS_MAP.fetch(status) do
      raise "Unknown #{name} order status: #{status}"
    end
  end

  # Scaffold for fetch_withdrawal_fees! on exchanges whose fee endpoint needs an
  # authenticated client: without a configured fee API key the fetch is skipped
  # as an empty success, otherwise yields a proxied Honeymaker client.
  def with_authenticated_fee_client(client_name, proxy_env)
    api_key = fee_api_key
    return Result::Success.new({}) if api_key.blank?

    yield Honeymaker.client(client_name,
                            api_key: api_key.key,
                            api_secret: api_key.secret,
                            proxy: ExchangeProxy.for(proxy_env.delete_prefix('PROXY_')))
  end

  def update_exchange_asset_fees!(fees, chains: {})
    updated = {}
    fees.each do |symbol, fee_string|
      asset = asset_from_symbol(symbol)
      next unless asset

      ea = exchange_assets.find_or_create_by!(asset: asset)
      attrs = { withdrawal_fee: fee_string, withdrawal_fee_updated_at: Time.current }
      attrs[:withdrawal_chains] = chains[symbol] if chains.key?(symbol)
      ea.update!(attrs)
      updated[symbol] = fee_string
    end
    Result::Success.new(updated)
  end
end
