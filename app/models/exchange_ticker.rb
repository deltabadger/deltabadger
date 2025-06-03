class ExchangeTicker < ApplicationRecord
  belongs_to :exchange
  belongs_to :base_asset, class_name: 'Asset'
  belongs_to :quote_asset, class_name: 'Asset'

  validates :exchange_id, uniqueness: { scope: %i[base_asset_id quote_asset_id] }
  validate :exchange_matches_assets

  include TechnicallyAnalyzable

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
    price.send(method, price_decimals)
  end

  private

  def exchange_matches_assets
    return if base_asset.exchanges.include?(exchange) && quote_asset.exchanges.include?(exchange)

    errors.add(:exchange, 'must match the exchange of base and quote assets')
  end
end
