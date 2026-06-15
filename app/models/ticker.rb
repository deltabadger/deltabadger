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

    begin
      result = public_send(method, force: force)
      result.success? && result.data.to_d.positive?
    rescue Client::TransientNetworkError
      raise
    rescue StandardError => e
      # Returning false is normal flow (e.g. a listed-but-dead pair with no live price),
      # not an error — keep at debug so it doesn't trip the log exception scanner.
      Rails.logger.debug("Ticker#priced? false for ticker=#{id} (#{ticker}) " \
                         "type=#{price_type}: #{e.class}: #{e.message}")
      false
    end
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
