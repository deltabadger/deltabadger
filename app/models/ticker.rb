class Ticker < ApplicationRecord
  belongs_to :exchange
  belongs_to :base_asset, class_name: 'Asset'
  belongs_to :quote_asset, class_name: 'Asset'

  validates :exchange_id, uniqueness: { scope: %i[base_asset_id quote_asset_id] }
  validate :exchange_matches_assets

  scope :available, -> { where(available: true) }
  scope :trading_enabled, -> { where(trading_enabled: true) }

  include Undeletable
  include TechnicallyAnalyzable

  # The venue's own name for the base asset (Kraken lists Bitcoin as XBT), without the prefix a replaced
  # listing carries.
  def base_spelling
    base.to_s.sub(/\A#{MarketData::TICKER_TOMBSTONE_PREFIX}\d+_/o, '')
  end

  # Every asset a venue lists under this name, by its spelling or by the asset's symbol.
  def self.asset_ids_named(exchange_id, name) = asset_ids_by_name(exchange_id, [name]).fetch(name.to_s.upcase, [])

  # { NAME => [asset ids] } for every name at once (upper-cased): two queries whatever the number of names.
  def self.asset_ids_by_name(exchange_id, names)
    wanted = names.compact_blank.map(&:upcase).uniq
    return {} if wanted.empty?

    found = {}
    add = ->(name, asset_id) { (found[name] ||= []) << asset_id unless found[name]&.include?(asset_id) }
    tombstoned = "#{sanitize_sql_like(MarketData::TICKER_TOMBSTONE_PREFIX)}%"
    where(exchange_id:).where("upper(tickers.base) IN (?) OR tickers.base LIKE ? ESCAPE '\\'", wanted, tombstoned)
                       .pluck(:base, :base_asset_id).each do |base, asset_id|
      spelling = new(base:).base_spelling.upcase
      add.call(spelling, asset_id) if wanted.include?(spelling)
    end
    where(exchange_id:).joins(:base_asset).where('upper(assets.symbol) IN (?)', wanted)
                       .pluck(Arel.sql('upper(assets.symbol)'), :base_asset_id).each { |name, asset_id| add.call(name, asset_id) }
    found
  end

  # Whether the pair currently has a live, non-zero market price for the given
  # price type (:ask, :bid, :last). Tolerates the exchange price methods raising
  # on a zero price.
  def priced?(price_type = :last, force: false)
    method = case price_type
             when :ask  then :get_ask_price
             when :bid  then :get_bid_price
             when :last then :get_last_price
             else raise ArgumentError, "Unsupported price_type: #{price_type.inspect}"
             end

    result = begin
      public_send(method, force: force)
    rescue Client::TransientNetworkError
      raise
    rescue StandardError => e
      # Returning false is normal flow (e.g. a listed-but-dead pair with no live price),
      # not an error — keep at debug so it doesn't trip the log exception scanner.
      Rails.logger.debug("Ticker#priced? false for ticker=#{id} (#{ticker}) " \
                         "type=#{price_type}: #{e.class}: #{e.message}")
      return false
    end

    # Rejected credentials are NOT "this pair has no live price": they make every ticker on the
    # exchange look unpriced at once, so a caller that only sees `false` reports a domain-shaped
    # lie — "No matching coins found on Alpaca for the index" — while the real fault is the key.
    # One such bot failed that way on every tick for six weeks. Escapes like a transient error
    # does, and for the same reason: the caller must be able to tell the difference.
    exchange.raise_on_invalid_key!(result)

    result.success? && result.data.to_d.positive?
  end

  # Whether the pair can actually be traded for the given order side right now:
  # the exchange reports trading enabled AND there's a live price on the side the
  # order will use (buy -> ask, sell -> bid). Order-side only by design; discovery
  # composes `trading_enabled` + `priced?` explicitly.
  def tradeable?(side, force: false)
    price_type = case side
                 when :buy  then :ask
                 when :sell then :bid
                 else raise ArgumentError, "Unsupported side: #{side.inspect}"
                 end

    trading_enabled? && priced?(price_type, force: force)
  end

  def get_last_price(force: false)
    exchange.get_last_price(ticker: self, force: force)
  end

  def get_bid_price(force: false)
    exchange.get_bid_price(ticker: self, force: force)
  end

  def get_ask_price(force: false)
    exchange.get_ask_price(ticker: self, force: force)
  end

  def get_candles(start_at:, timeframe:)
    exchange.get_candles(ticker: self, start_at: start_at, timeframe: timeframe)
  end

  def get_indicator_candles(start_at:, timeframe:)
    exchange.get_indicator_candles(ticker: self, start_at: start_at, timeframe: timeframe)
  end

  def restated_candles?
    exchange.restated_candles?(self)
  end

  def market_buy(amount:, amount_type:)
    exchange.market_buy(ticker: self, amount: amount, amount_type: amount_type)
  end

  def market_sell(amount:, amount_type:)
    exchange.market_sell(ticker: self, amount: amount, amount_type: amount_type)
  end

  def limit_buy(amount:, amount_type:, price:)
    exchange.limit_buy(ticker: self, amount: amount, amount_type: amount_type, price: price)
  end

  # @param amount_type [Symbol] :base or :quote
  def adjusted_amount(amount:, amount_type:, method: :floor)
    decimals = amount_type == :quote ? quote_decimals : base_decimals
    amount.send(method, decimals)
  end

  def adjusted_price(price:, method: :floor)
    exchange.adjusted_price(ticker: self, price:, method:)
  end

  private

  def exchange_matches_assets
    return if base_asset.exchanges.include?(exchange) && quote_asset.exchanges.include?(exchange)

    errors.add(:exchange, 'must match the exchange of base and quote assets')
  end
end
