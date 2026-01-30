class Exchange < ApplicationRecord
  include ExchangeApi::BinanceEnum

  has_many :bots
  has_many :api_keys
  has_many :exchange_assets
  has_many :assets, through: :exchange_assets
  has_many :tickers
  has_many :transactions

  validates :name, presence: true
  validates :type, uniqueness: true

  scope :available, -> { where(available: true) }

  include Synchronizer
  include CandleBuilder

  def symbols
    market = case name.downcase
             when 'binance' then ExchangeApi::Markets::Binance::Market.new(url_base: EU_URL_BASE)
             when 'binance.us' then ExchangeApi::Markets::Binance::Market.new(url_base: US_URL_BASE)
             when 'kraken' then ExchangeApi::Markets::Kraken::Market.new
             when 'coinbase' then ExchangeApi::Markets::Coinbase::Market.new
             else
               Result::Failure.new("Unsupported exchange #{name}")
             end
    cache_key = "#{name.downcase}_all_symbols"
    market.all_symbols(cache_key)
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

  def get_tickers_prices(force: false)
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

  def get_api_key_validity(api_key:)
    raise NotImplementedError, "#{self.class.name} must implement get_api_key_validity"
  end

  def minimum_amount_logic
    raise NotImplementedError, "#{self.class.name} must implement minimum_amount_logic"
  end
end
