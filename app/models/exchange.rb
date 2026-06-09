class Exchange < ApplicationRecord
  STABLE_TYPES = %w[Exchanges::Binance Exchanges::BinanceUs Exchanges::Coinbase Exchanges::Kraken].freeze

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
    'Errno::ECONNRESET'
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

  def beta?
    !type.in?(STABLE_TYPES)
  end

  def stock_venue?
    type.in?(STOCK_TYPES)
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

  def get_order(order_id:)
    raise NotImplementedError, "#{self.class.name} must implement get_order"
  end

  def get_orders(order_ids:)
    raise NotImplementedError, "#{self.class.name} must implement get_orders"
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
  def invalid_key_error?(errors)
    invalid_messages = (known_errors[:invalid_key] || []).map(&:to_s)
    return false if invalid_messages.empty?

    Array(errors).any? do |err|
      msg = err.to_s
      invalid_messages.any? { |m| msg.include?(m) }
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

  def market_open?
    true
  end

  def next_market_open_at
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
